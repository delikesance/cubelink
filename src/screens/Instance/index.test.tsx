import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import type { Instance, LaunchLogEvent, LaunchState, Services } from "../../services";
import { UNDO_MS } from "../Main/ModsPanel";
import InstancePage from "./index";

afterEach(cleanup);

const AERO = "Create: Aeronautics";

function setup(over: Partial<Services> = {}, params: Record<string, string> = { id: "inst-aeronautics" }) {
  const services = { ...createFakeServices({ tickMs: 0, requestCount: 0 }), modsCheck: async () => ({ issues: [], checked: 0 }), ...over } as ReturnType<typeof createFakeServices> & Partial<Services>;
  const navigate = vi.fn();
  const value = { services, route: "instance", params, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: true } as ShellContextValue;
  render(<ShellContext.Provider value={value}><InstancePage /></ShellContext.Provider>);
  return { services, navigate };
}

const ready = () => screen.findByRole("heading", { level: 1, name: AERO });
const tab = (name: string) => screen.getByRole("tab", { name });
const openMenu = async () => {
  await ready();
  fireEvent.click(screen.getByRole("button", { name: `Actions de ${AERO}` }));
  return screen.findByRole("menu");
};

describe("Page modpack", () => {
  it("affiche le nom, la ligne de méta et les 4 onglets (Aperçu actif)", async () => {
    setup();
    await ready();
    expect(screen.getByText(/Minecraft 1\.21\.1 · NeoForge .* · \d+ mods?/)).toBeTruthy();
    expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Aperçu", "Mods", "Réglages", "Logs"]);
    expect(tab("Aperçu").getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("button", { name: "Jouer" })).toBeTruthy();
  });

  it("id inconnu : message et retour à l'accueil", async () => {
    const { navigate } = setup({}, { id: "inconnu" });
    expect(await screen.findByRole("heading", { level: 1, name: "Modpack introuvable" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Retour à l'accueil" }));
    expect(navigate).toHaveBeenCalledWith("main");
  });

  it("fix=1 : onglet Mods et recherche des correctifs lancée", async () => {
    const modsFixPlan = vi.fn(async () => ({ plan_id: "p", actions: [], unresolved: [] }));
    setup({ modsFixPlan }, { id: "inst-aeronautics", tab: "mods", fix: "1" });
    await ready();
    expect(await screen.findByRole("dialog", { name: "Corrections proposées" })).toBeTruthy();
    expect(modsFixPlan).toHaveBeenCalledTimes(1);
  });

  it("lit l'onglet initial dans l'URL", async () => {
    setup({}, { id: "inst-aeronautics", tab: "mods" });
    await ready();
    expect(tab("Mods").getAttribute("aria-selected")).toBe("true");
    expect(await screen.findByRole("list", { name: "Mods installés" })).toBeTruthy();
  });

  it("onglets : clic, flèches, Début et Fin (tabindex itinérant)", async () => {
    setup();
    await ready();
    fireEvent.click(tab("Mods"));
    expect(tab("Mods").getAttribute("aria-selected")).toBe("true");
    expect(tab("Aperçu").getAttribute("tabindex")).toBe("-1");
    fireEvent.keyDown(tab("Mods"), { key: "ArrowRight" });
    expect(tab("Réglages").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tab("Réglages"));
    fireEvent.keyDown(tab("Réglages"), { key: "End" });
    expect(tab("Logs").getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(tab("Logs"), { key: "ArrowRight" });
    expect(tab("Aperçu").getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(tab("Aperçu"), { key: "ArrowLeft" });
    expect(tab("Logs").getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(tab("Logs"), { key: "Home" });
    expect(tab("Aperçu").getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("tabpanel").getAttribute("aria-labelledby")).toBe(tab("Aperçu").id);
  });
});

describe("Page modpack : lancement", () => {
  it("Jouer lance l'instance, puis Annuler, Arrêter, chronomètre et échec avec Voir les logs", async () => {
    let emit: (s: LaunchState) => void = () => {};
    const launch = vi.fn(async () => {});
    const kill = vi.fn(async () => {});
    setup({ instancesLaunch: launch, instancesKill: kill, onLaunchState: (cb) => { emit = cb; return () => {}; } });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer" }));
    await waitFor(() => expect(launch).toHaveBeenCalledWith("inst-aeronautics"));
    expect(screen.getByText("Préparation du jeu…")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Annuler le lancement" }));
    expect(kill).toHaveBeenCalledWith("inst-aeronautics");
    act(() => emit({ id: "inst-aeronautics", state: "running" }));
    expect(screen.getByText("En jeu")).toBeTruthy();
    expect(screen.getByText(/session/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Arrêter" }));
    expect(kill).toHaveBeenCalledTimes(2);
    act(() => emit({ id: "inst-aeronautics", state: "failed", message: "Java introuvable" }));
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Échec du lancement");
    expect(alert.textContent).toContain("Java introuvable");
    fireEvent.click(within(alert).getByRole("button", { name: "Voir les logs" }));
    expect(tab("Logs").getAttribute("aria-selected")).toBe("true");
  });

  it("l'échec de l'appel de lancement est annoncé", async () => {
    setup({ instancesLaunch: async () => { throw new Error("introuvable : pack"); } });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Échec du lancement");
  });
});

describe("Page modpack : amis sur ce pack", () => {
  it("liste les parties en cours sur ce pack et rejoint l'ami", async () => {
    const { services, navigate } = setup();
    await ready();
    expect(screen.getByText(/Personne n'y joue/)).toBeTruthy();
    const friend = (await services.friendList())[0].friend;
    act(() => { services.emitInvite({ public_key: friend.public_key, pack: AERO, world_name: "Monde test" }); });
    fireEvent.click(await screen.findByRole("button", { name: `Rejoindre ${friend.name}` }));
    expect(navigate).toHaveBeenCalledWith("join", { friend: friend.public_key });
  });

  it("ignore les parties d'un autre pack", async () => {
    const { services } = setup();
    await ready();
    const friend = (await services.friendList())[0].friend;
    act(() => { services.emitInvite({ public_key: friend.public_key, pack: "Autre pack" }); });
    await waitFor(() => expect(screen.getByText(/Personne n'y joue/)).toBeTruthy());
  });
});

describe("Page modpack : menu ⋯", () => {
  it("propose les 6 actions, Parcourir des mods navigue, Ouvrir le dossier appelle le service", async () => {
    const open = vi.fn(async () => {});
    const { navigate } = setup({ instancesOpenFolder: open });
    const menu = await openMenu();
    expect(within(menu).getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "Choisir une illustration", "Définir l'icône", "Chercher automatiquement", "Parcourir des mods", "Ouvrir le dossier", "Supprimer",
    ]);
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Parcourir des mods" }));
    expect(navigate).toHaveBeenCalledWith("browse", { kind: "mod", instance: "inst-aeronautics" });
    fireEvent.click(screen.getByRole("button", { name: `Actions de ${AERO}` }));
    fireEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Ouvrir le dossier" }));
    expect(open).toHaveBeenCalledWith("inst-aeronautics");
  });

  it("Échap ferme le menu et rend le focus au bouton", async () => {
    setup();
    await openMenu();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: `Actions de ${AERO}` }));
  });

  it("Définir l'icône : pickFiles puis instancesArtSet icon ; annulation sans appel", async () => {
    const withArt = (): Instance => ({
      id: "inst-aeronautics", name: AERO, minecraft: "1.21.1", loader: { loader: "neoforge", version: "21.1.1" }, ram_mb: 8192, mod_count: 94,
      art: { source: "custom", project_id: "p", title: "t", author: "a", summary: "", color: null, icon: "icon.png", screenshots: [], page_url: null },
    });
    const set = vi.fn(async () => withArt());
    const pick = vi.fn(async () => [] as string[]);
    setup({ instancesArtSet: set, pickFiles: pick });
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Définir l'icône" }));
    await waitFor(() => expect(pick).toHaveBeenCalled());
    expect(set).not.toHaveBeenCalled();
    pick.mockResolvedValueOnce(["C:\\img\\icon.png"]);
    fireEvent.click(screen.getByRole("button", { name: `Actions de ${AERO}` }));
    fireEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Définir l'icône" }));
    await waitFor(() => expect(set).toHaveBeenCalledWith("inst-aeronautics", "C:\\img\\icon.png", "icon"));
    expect(await screen.findByText("Icône définie.")).toBeTruthy();
  });

  it("Chercher automatiquement : l'erreur est annoncée", async () => {
    setup();
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Chercher automatiquement" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/Aucun modpack correspondant/);
  });
});

describe("Page modpack : suppression", () => {
  it("confirmation (alertdialog) : Annuler ne supprime rien", async () => {
    const del = vi.fn(async () => {});
    const { navigate } = setup({ instancesDelete: del });
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Supprimer" }));
    const dlg = await screen.findByRole("alertdialog");
    fireEvent.click(within(dlg).getByRole("button", { name: "Annuler" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(del).not.toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("Supprimer confirmé : instancesDelete puis navigate(main)", async () => {
    const del = vi.fn(async () => {});
    const { navigate } = setup({ instancesDelete: del });
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Supprimer" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Supprimer" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
    expect(del).toHaveBeenCalledWith("inst-aeronautics");
  });

  it("échec de la suppression : erreur affichée, pas de navigation", async () => {
    const { navigate } = setup({ instancesDelete: async () => { throw new Error("fichier verrouillé"); } });
    fireEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Supprimer" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Supprimer" }));
    expect((await screen.findByRole("alert")).textContent).toContain("fichier verrouillé");
    expect(navigate).not.toHaveBeenCalled();
  });
});

describe("Page modpack : onglet Mods", () => {
  async function openMods() {
    const view = setup({}, { id: "inst-aeronautics", tab: "mods" });
    await ready();
    const list = await screen.findByRole("list", { name: "Mods installés" });
    return { ...view, panel: screen.getByRole("tabpanel"), list };
  }

  it("liste les mods, sans bouton Fermer, et met à jour le compteur de l'en-tête", async () => {
    const { panel } = await openMods();
    expect(within(panel).getAllByRole("listitem")).toHaveLength(9);
    expect(within(panel).getByText("Personnalisé")).toBeTruthy();
    expect(within(panel).getByText(/9 mods · 8 activés/)).toBeTruthy();
    expect(within(panel).queryByRole("button", { name: "Fermer" })).toBeNull();
    expect(screen.getByText(/· 9 mods$/)).toBeTruthy();
  });

  it("filtre par nom", async () => {
    const { panel } = await openMods();
    fireEvent.change(within(panel).getByRole("searchbox"), { target: { value: "sodium" } });
    expect(within(panel).getAllByRole("listitem")).toHaveLength(1);
    fireEvent.change(within(panel).getByRole("searchbox"), { target: { value: "zzz" } });
    expect(within(panel).getByText("Aucun mod ne correspond à la recherche.")).toBeTruthy();
  });

  it("l'interrupteur active/désactive le mod", async () => {
    const { panel, services } = await openMods();
    fireEvent.click(within(panel).getByRole("switch", { name: "Désactiver sodium-1.1.0.jar" }));
    await waitFor(() => expect(within(panel).getByRole("switch", { name: "Activer sodium-1.1.0.jar" }).getAttribute("aria-checked")).toBe("false"));
    expect((await services.modsList("inst-aeronautics")).find((m) => m.file === "sodium-1.1.0.jar")?.enabled).toBe(false);
  });

  it("suppression annulable : Annuler rétablit le mod, puis suppression appliquée après le délai", async () => {
    const { panel, services } = await openMods();
    const rm = vi.spyOn(services, "modsRemove");
    fireEvent.click(within(panel).getByRole("button", { name: "Supprimer mod-maison.jar" }));
    fireEvent.click(within(panel).getByRole("button", { name: /^Annuler \(\d+ s\)$/ }));
    expect(within(panel).getByText("mod-maison.jar")).toBeTruthy();
    expect(rm).not.toHaveBeenCalled();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      fireEvent.click(within(panel).getByRole("button", { name: "Supprimer mod-maison.jar" }));
      await act(async () => { await vi.advanceTimersByTimeAsync(UNDO_MS + 50); });
    } finally { vi.useRealTimers(); }
    await waitFor(() => expect(within(panel).queryByText("mod-maison.jar")).toBeNull());
    expect(await services.modsList("inst-aeronautics")).toHaveLength(8);
  });

  it("ajoute des mods (bouton) et les .jar déposés, refuse les autres fichiers", async () => {
    const { panel, services } = await openMods();
    services.setPickedFiles(["C:\\x\\nouveau.jar"]);
    fireEvent.click(within(panel).getByRole("button", { name: "Ajouter un .jar" }));
    expect(await within(panel).findByText("nouveau.jar")).toBeTruthy();
    act(() => services.dropFiles(["/tmp/a.jar", "/tmp/notes.txt"]));
    expect(await within(panel).findByText("a.jar")).toBeTruthy();
    expect(within(panel).getByText(/refusé.*notes\.txt/)).toBeTruthy();
  });

  it("« Parcourir des mods » navigue vers browse avec l'instance", async () => {
    const { panel, navigate } = await openMods();
    fireEvent.click(within(panel).getByRole("button", { name: "Parcourir des mods" }));
    expect(navigate).toHaveBeenCalledWith("browse", { kind: "mod", instance: "inst-aeronautics" });
  });
});

describe("Page modpack : onglet Réglages", () => {
  it("enregistre nom, RAM, Java et JVM via instancesUpdate et reste sur la page", async () => {
    const update = vi.fn(async (id: string, patch: object) => ({
      id, name: "Renommé", minecraft: "1.21.1", loader: { loader: "neoforge" as const, version: "1" }, ram_mb: 8192, mod_count: 94, ...patch,
    }));
    setup({ instancesUpdate: update });
    await ready();
    fireEvent.click(tab("Réglages"));
    const panel = screen.getByRole("tabpanel");
    expect(within(panel).queryByRole("button", { name: "Annuler" })).toBeNull();
    fireEvent.change(within(panel).getByLabelText("Nom de l'instance"), { target: { value: "Renommé" } });
    fireEvent.change(within(panel).getByLabelText("Mémoire allouée"), { target: { value: "8" } });
    fireEvent.change(within(panel).getByLabelText(/Chemin de Java/), { target: { value: "C:\\java\\bin\\java.exe" } });
    fireEvent.change(within(panel).getByLabelText("Arguments JVM"), { target: { value: "-XX:+UseG1GC" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Enregistrer" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith("inst-aeronautics", {
      name: "Renommé", ram_mb: 8192, java_path: "C:\\java\\bin\\java.exe", jvm_args: "-XX:+UseG1GC",
    }));
    expect(await screen.findByRole("heading", { level: 1, name: "Renommé" })).toBeTruthy();
    expect(await within(screen.getByRole("tabpanel")).findByText("Réglages enregistrés.")).toBeTruthy();
  });

  it("valide le nom et affiche l'erreur du service", async () => {
    const update = vi.fn(async () => { throw new Error("disque plein"); });
    setup({ instancesUpdate: update });
    await ready();
    fireEvent.click(tab("Réglages"));
    const panel = screen.getByRole("tabpanel");
    fireEvent.change(within(panel).getByLabelText("Nom de l'instance"), { target: { value: "" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Enregistrer" }));
    expect(update).not.toHaveBeenCalled();
    fireEvent.change(within(panel).getByLabelText("Nom de l'instance"), { target: { value: "Valide" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Enregistrer" }));
    expect((await within(panel).findByText("disque plein"))).toBeTruthy();
  });

  it("zone dangereuse : confirmation intégrée puis suppression et retour à l'accueil", async () => {
    const del = vi.fn(async () => {});
    const { navigate } = setup({ instancesDelete: del });
    await ready();
    fireEvent.click(tab("Réglages"));
    const panel = screen.getByRole("tabpanel");
    fireEvent.click(within(panel).getByRole("button", { name: "Supprimer l'instance" }));
    fireEvent.click(within(within(panel).getByRole("alertdialog")).getByRole("button", { name: "Supprimer" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
    expect(del).toHaveBeenCalledWith("inst-aeronautics");
  });
});

describe("Page modpack : onglet Logs", () => {
  it("charge l'historique, suit onLaunchLog et copie", async () => {
    let push: (e: LaunchLogEvent) => void = () => {};
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    setup({ instancesLogs: async () => "ligne 1\nligne 2", onLaunchLog: (cb) => { push = cb; return () => {}; } });
    await ready();
    fireEvent.click(tab("Logs"));
    const log = await screen.findByRole("log", { name: "Journal du jeu" });
    const rows = () => Array.from(log.querySelectorAll(".dr-line")).map((r) => r.textContent);
    await waitFor(() => expect(rows()).toEqual(["ligne 1", "ligne 2"]));
    act(() => push({ id: "inst-aeronautics", line: "ligne 3" }));
    act(() => push({ id: "autre", line: "ignorée" }));
    expect(rows()).toEqual(["ligne 1", "ligne 2", "ligne 3"]);
    fireEvent.click(screen.getByRole("button", { name: "Copier les logs" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("ligne 1\nligne 2\nligne 3"));
    expect(await screen.findByRole("button", { name: "Logs copiés" })).toBeTruthy();
    // Essentiel seulement : ni « Vider l'affichage » ni « Suivre en direct » (le panneau suit le bas tout seul).
    expect(screen.queryByRole("button", { name: "Vider l'affichage" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Suivre en direct" })).toBeNull();
  });

  it("recherche : filtre les lignes, surligne, compte, Échap efface", async () => {
    setup({ instancesLogs: async () => "[10:00:00] [main/INFO]: Loaded Sodium\n[10:00:01] [main/INFO]: autre chose\n[10:00:02] [main/WARN]: sodium lent" }, { id: "inst-aeronautics", tab: "logs" });
    const log = await screen.findByRole("log", { name: "Journal du jeu" });
    await waitFor(() => expect(log.querySelectorAll(".dr-line")).toHaveLength(3));
    const box = screen.getByRole("searchbox", { name: "Rechercher dans les logs" });
    fireEvent.change(box, { target: { value: "SODIUM" } });
    await waitFor(() => expect(log.querySelectorAll(".dr-line")).toHaveLength(2));
    expect(Array.from(log.querySelectorAll("mark")).map((m) => m.textContent)).toEqual(["Sodium", "sodium"]);
    expect(screen.getByText("2 résultats")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Avertissements (1)" }));
    await waitFor(() => expect(log.querySelectorAll(".dr-line")).toHaveLength(1));
    fireEvent.change(box, { target: { value: "introuvable" } });
    expect(await screen.findByText("Aucune ligne ne contient « introuvable ».")).toBeTruthy();
    fireEvent.keyDown(box, { key: "Escape" });
    expect((box as HTMLInputElement).value).toBe("");
  });

  it("filtre par niveau", async () => {
    setup({ instancesLogs: async () => "[10:00:00] [main/INFO]: ok\n[10:00:01] [main/ERROR]: boom" }, { id: "inst-aeronautics", tab: "logs" });
    const log = await screen.findByRole("log", { name: "Journal du jeu" });
    await waitFor(() => expect(log.querySelectorAll(".dr-line")).toHaveLength(2));
    // Compteurs par niveau sur les boutons du filtre.
    expect(screen.getByRole("button", { name: "Avertissements (0)" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Erreurs (1)" }));
    expect(log.querySelectorAll(".dr-line")).toHaveLength(1);
    expect(log.querySelector(".dr-line.is-error")).toBeTruthy();
  });
});
