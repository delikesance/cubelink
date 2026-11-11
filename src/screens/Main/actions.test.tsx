import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { setImportWarning } from "../../app/importWarning";
import { createFakeServices } from "../../services/fake";
import type { LaunchLogEvent, LaunchState, Services } from "../../services";
import Main from "./index";

afterEach(() => { cleanup(); setImportWarning(null); });

function setup(over: Partial<Services> = {}) {
  const services = { ...createFakeServices({ tickMs: 0, requestCount: 0 }), ...over };
  const navigate = vi.fn();
  const value = { services, route: "main", params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Main /></ShellContext.Provider>);
  return { services };
}

const rail = () => screen.getByRole("navigation", { name: "Instances" });
const openMenu = async () => {
  await screen.findByRole("heading", { level: 1, name: "Create: Aeronautics" });
  fireEvent.contextMenu(within(rail()).getByRole("button", { name: "Create: Aeronautics" }));
  return screen.findByRole("menu");
};

describe("Main : menu contextuel", () => {
  it("s'ouvre au clic droit avec les 10 actions, focus sur la première", async () => {
    setup();
    const menu = await openMenu();
    const items = within(menu).getAllByRole("menuitem").map((i) => i.textContent);
    expect(items).toEqual(["Jouer", "Paramètres", "Logs", "Mods", "Parcourir des mods", "Choisir une illustration", "Définir l'icône", "Chercher automatiquement", "Ouvrir le dossier", "Supprimer"]);
    expect(document.activeElement).toBe(within(menu).getByRole("menuitem", { name: "Jouer" }));
  });

  it("flèches, Début/Fin, puis Échap ferme et rend le focus", async () => {
    setup();
    await screen.findByRole("heading", { level: 1, name: "Create: Aeronautics" });
    const trigger = within(rail()).getByRole("button", { name: "Create: Aeronautics" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "F10", shiftKey: true });
    const menu = await screen.findByRole("menu");
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(within(menu).getByRole("menuitem", { name: "Paramètres" }));
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(document.activeElement).toBe(within(menu).getByRole("menuitem", { name: "Supprimer" }));
    fireEvent.keyDown(menu, { key: "Home" });
    expect(document.activeElement).toBe(within(menu).getByRole("menuitem", { name: "Jouer" }));
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("s'ouvre avec la touche Menu sur « ⋯ » et au clic, se ferme au clic extérieur", async () => {
    setup();
    const more = await screen.findByRole("button", { name: "Actions de Create: Aeronautics" });
    expect(more.getAttribute("aria-haspopup")).toBe("menu");
    fireEvent.keyDown(more, { key: "ContextMenu" });
    expect(await screen.findByRole("menu")).toBeTruthy();
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("menu")).toBeNull();
    fireEvent.click(more);
    expect(await screen.findByRole("menu")).toBeTruthy();
  });

  it("Jouer lance l'instance, Ouvrir le dossier appelle le service", async () => {
    const launch = vi.fn(async () => {});
    const open = vi.fn(async () => {});
    setup({ instancesLaunch: launch, instancesOpenFolder: open });
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Jouer" }));
    expect(launch).toHaveBeenCalledWith("inst-aeronautics");
    expect(screen.queryByRole("menu")).toBeNull();
    fireEvent.contextMenu(within(rail()).getByRole("button", { name: "Create: Aeronautics" }));
    expect(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Arrêter" })).toBeTruthy();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    fireEvent.contextMenu(within(rail()).getByRole("button", { name: "Create: Aeronautics" }));
    fireEvent.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Ouvrir le dossier" }));
    expect(open).toHaveBeenCalledWith("inst-aeronautics");
  });
});

describe("Main : suppression", () => {
  it("demande confirmation (alertdialog), Annuler ne supprime rien, Échap annule", async () => {
    const del = vi.fn(async () => {});
    setup({ instancesDelete: del });
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Supprimer" }));
    const dlg = await screen.findByRole("alertdialog");
    expect(document.activeElement).toBe(within(dlg).getByRole("button", { name: "Annuler" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(del).not.toHaveBeenCalled();
  });

  it("Supprimer confirmé retire l'instance", async () => {
    setup();
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Supprimer" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Supprimer" }));
    // La fake contient plusieurs instances : la suivante est sélectionnée, l'instance supprimée disparaît.
    expect(await screen.findByRole("heading", { level: 1, name: "All the Mods 10" })).toBeTruthy();
    expect(within(rail()).queryByRole("button", { name: "Create: Aeronautics" })).toBeNull();
  });

  it("affiche l'erreur si la suppression échoue", async () => {
    setup({ instancesDelete: async () => { throw new Error("Dossier verrouillé."); } });
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Supprimer" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Supprimer" }));
    expect(await screen.findByText("Dossier verrouillé.")).toBeTruthy();
    expect(screen.getByRole("heading", { level: 1, name: "Create: Aeronautics" })).toBeTruthy();
  });
});

describe("Main : Jouer / Arrêter", () => {
  it("Jouer devient Arrêter (preparing puis running) et appelle instancesKill", async () => {
    let emit: (s: LaunchState) => void = () => {};
    const kill = vi.fn(async () => {});
    setup({ instancesKill: kill, onLaunchState: (cb) => { emit = cb; return () => {}; } });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer" }));
    expect(screen.getByRole("button", { name: "Annuler le lancement" })).toBeTruthy();
    act(() => emit({ id: "inst-aeronautics", state: "running" }));
    expect(screen.getByRole("button", { name: "Arrêter" }).className).toContain("is-danger");
    fireEvent.click(screen.getByRole("button", { name: "Arrêter" }));
    expect(kill).toHaveBeenCalledWith("inst-aeronautics");
    act(() => emit({ id: "inst-aeronautics", state: "exited" }));
    expect(screen.getByRole("button", { name: "Jouer" })).toBeTruthy();
  });
});

describe("Main : indice hôte", () => {
  it("pendant le jeu : invite à ouvrir le monde, puis annonce le partage actif", async () => {
    let emit: (s: LaunchState) => void = () => {};
    const { services } = setup({ onLaunchState: (cb) => { emit = cb; return () => {}; } });
    await screen.findByRole("button", { name: "Jouer" });
    expect(screen.queryByText(/réseau local/)).toBeNull();
    act(() => emit({ id: "inst-aeronautics", state: "running" }));
    expect(screen.getByText("Ouvre ton monde au réseau local pour que tes amis te rejoignent")).toBeTruthy();
    act(() => services.emitHostState({ instance_id: "inst-aeronautics", sharing: true, world_port: 25565 }));
    expect(screen.getByText("Tes amis peuvent te rejoindre").getAttribute("role")).toBe("status");
    act(() => services.emitHostState({ instance_id: "autre", sharing: true, world_port: 25565 }));
    expect(screen.getByText(/Ouvre ton monde/)).toBeTruthy();
  });
});

describe("Main : paramètres", () => {
  it("enregistre nom, RAM, Java et arguments JVM via instancesUpdate", async () => {
    const update = vi.fn(async (id: string, patch: object) => ({
      id, name: "Renommé", minecraft: "1.21.1", loader: { loader: "neoforge" as const, version: "1" }, ram_mb: 8192, mod_count: 94, ...patch,
    }));
    setup({ instancesUpdate: update });
    fireEvent.click(await screen.findByRole("button", { name: "Paramètres" }));
    const dlg = await screen.findByRole("dialog", { name: "Paramètres de Create: Aeronautics" });
    fireEvent.change(within(dlg).getByLabelText("Nom de l'instance"), { target: { value: "Renommé" } });
    fireEvent.change(within(dlg).getByLabelText("Mémoire allouée"), { target: { value: "8" } });
    fireEvent.change(within(dlg).getByLabelText(/Chemin de Java/), { target: { value: "C:\\java\\bin\\java.exe" } });
    fireEvent.change(within(dlg).getByLabelText("Arguments JVM"), { target: { value: "-XX:+UseG1GC" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Enregistrer" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(update).toHaveBeenCalledWith("inst-aeronautics", {
      name: "Renommé", ram_mb: 8192, java_path: "C:\\java\\bin\\java.exe", jvm_args: "-XX:+UseG1GC",
    });
    expect(screen.getByRole("heading", { level: 1, name: "Renommé" })).toBeTruthy();
  });

  it("valide le nom et affiche l'erreur du service", async () => {
    setup({ instancesUpdate: async () => { throw new Error("Échec d'écriture."); } });
    fireEvent.click(await screen.findByRole("button", { name: "Paramètres" }));
    const dlg = await screen.findByRole("dialog");
    fireEvent.change(within(dlg).getByLabelText("Nom de l'instance"), { target: { value: "  " } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Enregistrer" }));
    expect((await within(dlg).findByRole("alert")).textContent).toContain("obligatoire");
    fireEvent.change(within(dlg).getByLabelText("Nom de l'instance"), { target: { value: "Ok" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Enregistrer" }));
    expect(await within(dlg).findByText("Échec d'écriture.")).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: "Annuler" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("zone dangereuse : confirmation intégrée puis suppression", async () => {
    const del = vi.fn(async () => {});
    setup({ instancesDelete: del });
    fireEvent.click(await screen.findByRole("button", { name: "Paramètres" }));
    const dlg = await screen.findByRole("dialog");
    fireEvent.click(within(dlg).getByRole("button", { name: "Supprimer l'instance" }));
    fireEvent.click(within(within(dlg).getByRole("alertdialog")).getByRole("button", { name: "Garder" }));
    expect(within(dlg).queryByRole("alertdialog")).toBeNull();
    expect(del).not.toHaveBeenCalled();
    fireEvent.click(within(dlg).getByRole("button", { name: "Supprimer l'instance" }));
    fireEvent.click(within(within(dlg).getByRole("alertdialog")).getByRole("button", { name: "Supprimer" }));
    await waitFor(() => expect(del).toHaveBeenCalledWith("inst-aeronautics"));
  });
});

describe("Main : logs", () => {
  it("charge l'historique, suit onLaunchLog, copie et vide l'affichage", async () => {
    let push: (e: LaunchLogEvent) => void = () => {};
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    setup({
      instancesLogs: async () => "ligne 1\nligne 2",
      onLaunchLog: (cb) => { push = cb; return () => {}; },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Logs" }));
    const log = await screen.findByRole("log", { name: "Journal du jeu" });
    const rows = () => Array.from(log.querySelectorAll(".dr-line")).map((r) => r.textContent);
    await waitFor(() => expect(rows()).toEqual(["ligne 1", "ligne 2"]));
    act(() => push({ id: "inst-aeronautics", line: "ligne 3" }));
    act(() => push({ id: "autre", line: "ignorée" }));
    expect(rows()).toEqual(["ligne 1", "ligne 2", "ligne 3"]);
    fireEvent.click(screen.getByRole("button", { name: "Copier" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("ligne 1\nligne 2\nligne 3"));
    fireEvent.click(screen.getByRole("button", { name: "Vider l'affichage" }));
    expect(log.textContent).toBe("Aucune ligne à afficher.");
    fireEvent.keyDown(log, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("filtre par niveau et colore les lignes d'erreur", async () => {
    setup({
      instancesLogs: async () => "[21:14:02] [main/INFO]: Démarrage\n[21:14:03] [main/WARN]: Attention\n[21:14:04] [Render thread/ERROR]: Boum",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Logs" }));
    const log = await screen.findByRole("log", { name: "Journal du jeu" });
    await waitFor(() => expect(log.querySelectorAll(".dr-line").length).toBe(3));
    fireEvent.click(screen.getByRole("button", { name: "Erreurs" }));
    expect(log.querySelectorAll(".dr-line").length).toBe(1);
    expect(log.querySelector(".dr-line.is-error")?.textContent).toContain("Boum");
    fireEvent.click(screen.getByRole("button", { name: "Avertissements" }));
    expect(log.querySelector(".dr-line.is-warn")?.textContent).toContain("Attention");
  });
});

describe("Main : avertissement d'import", () => {
  it("affiche un bandeau non bloquant pour les mods manquants", async () => {
    let push: (e: { missing: string[] }) => void = () => {};
    setup({ onImportWarning: (cb) => { push = cb; return () => {}; } });
    await screen.findByRole("heading", { level: 1, name: "Create: Aeronautics" });
    act(() => push({ missing: ["sodium", "iris"] }));
    expect(screen.getByText(/2 mods introuvables : sodium, iris/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Ignorer" }));
    expect(screen.queryByText(/introuvables/)).toBeNull();
  });

  it("reprend l'avertissement reçu pendant l'import (écran Create)", async () => {
    setImportWarning(["jei"]);
    setup();
    expect(await screen.findByText(/1 mod introuvable : jei/)).toBeTruthy();
  });
});
