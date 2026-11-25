import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services/types";
import { describeDetected, validateName } from "./index";

beforeEach(() => { window.location.hash = "#/create"; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const nameInput = () => screen.getByLabelText("Nom") as HTMLInputElement;
const createBtn = () => screen.getByRole("button", { name: /Créer l'instance/ });

/** Écran de choix (B3). */
async function chooser(svc: Services) {
  render(<Shell services={svc} />);
  await screen.findByRole("heading", { name: "Créer une instance" });
}
/** Formulaire (B4) : version recommandée chargée. */
async function form(svc: Services) {
  await chooser(svc);
  fireEvent.click(screen.getByRole("button", { name: /Créer une instance vide/ }));
  await waitFor(() => expect(screen.getByLabelText("Version de Minecraft").textContent).toBe("1.21.1 (recommandée)"));
}

describe("validateName", () => {
  it("règles", () => {
    expect(validateName("  ")).toMatch(/obligatoire/);
    expect(validateName("a".repeat(65))).toMatch(/64/);
    expect(validateName("a:b")).toMatch(/caractères/);
    expect(validateName("Create: Aeronautics".replace(":", ""))).toBeNull();
  });
});

describe("écran Créer : choix (B3)", () => {
  it("propose les quatre chemins", async () => {
    await chooser(createFakeServices({ tickMs: 0 }));
    expect(screen.getByRole("button", { name: /Parcourir les modpacks/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Créer une instance vide/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Importer un fichier/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Importer depuis un autre lanceur/ })).toBeTruthy();
  });

  it("« Parcourir les modpacks » ouvre l'écran Parcourir (modpacks)", async () => {
    await chooser(createFakeServices({ tickMs: 0 }));
    fireEvent.click(screen.getByRole("button", { name: /Parcourir les modpacks/ }));
    await waitFor(() => expect(window.location.hash).toContain("#/browse"));
    expect(window.location.hash).toContain("kind=modpack");
  });

  it("import : fichier choisi puis instancesImportMrpack", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const imp = vi.fn(base.instancesImportMrpack);
    await chooser({ ...base, instancesImportMrpack: imp, pickFiles: async () => ["C:\\p\\a.mrpack"] });
    fireEvent.click(screen.getByRole("button", { name: /Importer un fichier/ }));
    await waitFor(() => expect(window.location.hash).toMatch(/^#\/instance\?id=inst-/));
    expect(imp).toHaveBeenCalledWith("C:\\p\\a.mrpack", expect.objectContaining({ loader: expect.anything() }));
  });

  it("import dont la préparation du jeu échoue : instance gardée, retour à l'accueil avec l'avis", async () => {
    const base = createFakeServices({ tickMs: 0 });
    await chooser({ ...base, pickFiles: async () => ["C:\\p\\pack-differe.mrpack"] });
    fireEvent.click(screen.getByRole("button", { name: /Importer un fichier/ }));
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
    expect(await screen.findByText(/le jeu n'a pas pu être préparé \(réseau indisponible\)/)).toBeTruthy();
    expect(screen.getByText(/premier lancement/)).toBeTruthy();
    expect((await base.instancesList()).some((i) => i.name === "Pack importé")).toBe(true);
  });

  it("import : si le sélecteur échoue, saisie du chemin à la main", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const imp = vi.fn(base.instancesImportMrpack);
    await chooser({ ...base, instancesImportMrpack: imp, pickFiles: () => Promise.reject(new Error("Pas de dialogue.")) });
    fireEvent.click(screen.getByRole("button", { name: /Importer un fichier/ }));
    fireEvent.change(await screen.findByLabelText("Chemin du modpack (.mrpack ou .zip)"), { target: { value: "C:\\p\\a.mrpack" } });
    fireEvent.click(screen.getByRole("button", { name: "Importer" }));
    await waitFor(() => expect(window.location.hash).toMatch(/^#\/instance\?id=inst-/));
    expect(imp).toHaveBeenCalled();
  });

  it("Annuler et Échap reviennent à main", async () => {
    await chooser(createFakeServices({ tickMs: 0 }));
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(window.location.hash).toBe("#/main");
    window.location.hash = "#/create";
    cleanup();
    await chooser(createFakeServices({ tickMs: 0 }));
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(window.location.hash).toBe("#/main");
  });
});

describe("écran Créer : formulaire (B4)", () => {
  it("1.21.1 sélectionnée, NeoForge par défaut, Nom focalisé", async () => {
    await form(createFakeServices({ tickMs: 0 }));
    expect(screen.getByLabelText("Version de Minecraft").textContent).toContain("1.21.1");
    expect((screen.getByRole("radio", { name: "NeoForge" }) as HTMLInputElement).checked).toBe(true);
    expect(document.activeElement).toBe(nameInput());
  });

  it("création réussie : instancesCreate puis page de la nouvelle instance", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const create = vi.fn(base.instancesCreate);
    await form({ ...base, instancesCreate: create });
    fireEvent.change(nameInput(), { target: { value: "Mon monde" } });
    fireEvent.click(screen.getByRole("radio", { name: "Fabric" }));
    fireEvent.click(createBtn());
    await waitFor(() => expect(window.location.hash).toMatch(/^#\/instance\?id=inst-/));
    expect(create).toHaveBeenCalledWith({
      name: "Mon monde", minecraft: "1.21.1", loader: { loader: "fabric", version: "" }, ram_mb: 0,
    });
  });

  it("id absent de la réponse : retour à main", async () => {
    const base = createFakeServices({ tickMs: 0 });
    await form({ ...base, instancesCreate: async (i) => ({ ...(await base.instancesCreate(i)), id: "" }) });
    fireEvent.change(nameInput(), { target: { value: "Sans id" } });
    fireEvent.click(createBtn());
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
  });

  it("options avancées : la mémoire modifiée est transmise", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const create = vi.fn(base.instancesCreate);
    await form({ ...base, instancesCreate: create });
    fireEvent.change(nameInput(), { target: { value: "Gros pack" } });
    fireEvent.click(screen.getByRole("button", { name: "Options avancées" }));
    // Par défaut : valeur des paramètres ; décocher pour choisir une valeur propre.
    const inherit = screen.getByRole("checkbox", { name: /Valeur des paramètres/ }) as HTMLInputElement;
    expect(inherit.checked).toBe(true);
    fireEvent.click(inherit);
    fireEvent.change(screen.getByLabelText("Mémoire allouée (Go)"), { target: { value: "8" } });
    fireEvent.click(createBtn());
    await waitFor(() => expect(window.location.hash).toMatch(/^#\/instance\?id=inst-/));
    expect(create).toHaveBeenCalledWith(expect.objectContaining({ ram_mb: 8192 }));
  });

  it("nom invalide : message lié au champ, aucun appel", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const create = vi.fn(base.instancesCreate);
    await form({ ...base, instancesCreate: create });
    fireEvent.click(createBtn());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/obligatoire/);
    expect(nameInput().getAttribute("aria-describedby")).toContain(alert.id);
    fireEvent.change(nameInput(), { target: { value: "a/b" } });
    fireEvent.click(createBtn());
    expect((await screen.findByRole("alert")).textContent).toMatch(/caractères/);
    expect(create).not.toHaveBeenCalled();
    expect(window.location.hash).toBe("#/create");
  });

  it("« Autre méthode » revient au choix", async () => {
    await form(createFakeServices({ tickMs: 0 }));
    fireEvent.click(screen.getByRole("button", { name: "Autre méthode" }));
    expect(await screen.findByRole("button", { name: /Parcourir les modpacks/ })).toBeTruthy();
  });

  it("erreur du service affichée, écran conservé", async () => {
    const base = createFakeServices({ tickMs: 0 });
    await form({ ...base, instancesCreate: () => Promise.reject(new Error("Disque plein.")) });
    fireEvent.change(nameInput(), { target: { value: "Test" } });
    fireEvent.click(createBtn());
    expect((await screen.findByRole("alert")).textContent).toContain("Disque plein.");
    expect(window.location.hash).toBe("#/create");
    await waitFor(() => expect((createBtn() as HTMLButtonElement).disabled).toBe(false));
  });
});

describe("écran Créer : importer depuis un autre lanceur (#138)", () => {
  const open = async (svc: Services) => {
    await chooser(svc);
    fireEvent.click(screen.getByRole("button", { name: /Importer depuis un autre lanceur/ }));
  };

  it("décrit l'instance : version, loader, mods, mondes", () => {
    const base = { launcher: "prism" as const, name: "X", path: "p", has_saves: false, unsupported: null };
    expect(describeDetected({ ...base, minecraft: "1.21.1", loader: { loader: "fabric", version: "0.16.5" }, mod_count: 42, has_saves: true }))
      .toBe("Minecraft 1.21.1 · Fabric 0.16.5 · 42 mods · avec ses mondes");
    expect(describeDetected({ ...base, minecraft: "1.20.4", loader: { loader: "vanilla", version: "" }, mod_count: 1 })).toBe("Minecraft 1.20.4 · Sans mods · 1 mod");
    expect(describeDetected({ ...base, minecraft: "1.20.4", loader: { loader: "forge", version: "" }, mod_count: 0 })).toBe("Minecraft 1.20.4 · Forge");
  });

  it("liste les instances trouvées, avec le lanceur ; un loader non géré n'est pas importable", async () => {
    await open(createFakeServices({ tickMs: 0 }));
    const list = await screen.findByRole("list", { name: "Instances trouvées" });
    expect(list.querySelectorAll("li")).toHaveLength(4);
    expect(screen.getByText("Aventure entre amis")).toBeTruthy();
    expect(screen.getAllByText("Prism", { selector: ".cl-badge" })).toHaveLength(2);
    expect(screen.getByText("CurseForge", { selector: ".cl-badge" })).toBeTruthy();
    expect(screen.getByText("Modrinth", { selector: ".cl-badge" })).toBeTruthy();
    expect(screen.getByText(/Minecraft 1\.21\.1 · Fabric 0\.16\.5 · 42 mods · avec ses mondes/)).toBeTruthy();
    expect(screen.getByText(/liteloader/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Importer Vieux pack" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Importer Aventure entre amis" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("importer une instance appelle launcherImportRun avec son dossier et ouvre l'instance créée", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const run = vi.fn(base.launcherImportRun);
    await open({ ...base, launcherImportRun: run });
    fireEvent.click(await screen.findByRole("button", { name: "Importer All the Mods 10" }));
    await waitFor(() => expect(window.location.hash).toMatch(/^#\/instance\?id=inst-/));
    expect(run).toHaveBeenCalledWith(expect.stringContaining("ATM10"), "curseforge", expect.anything());
  });

  it("une erreur d'import s'affiche et laisse recommencer", async () => {
    const base = createFakeServices({ tickMs: 0 });
    await open({ ...base, launcherImportRun: () => Promise.reject(new Error("Pas assez de place sur le disque.")) });
    fireEvent.click(await screen.findByRole("button", { name: "Importer Survie vanilla" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Pas assez de place sur le disque.");
    expect(window.location.hash).toBe("#/create");
    expect((screen.getByRole("button", { name: "Importer Survie vanilla" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("rien de détecté : message, sélecteur de dossier puis liste du dossier choisi", async () => {
    const instance = { launcher: "prism" as const, name: "Choisie", path: "D:\\Jeux\\Choisie", minecraft: "1.21.1", loader: { loader: "vanilla" as const, version: "" }, mod_count: 3, has_saves: false, unsupported: null };
    const base = createFakeServices({ tickMs: 0, detectedInstances: [], manualInstances: [instance] });
    const scan = vi.fn(base.launcherImportScan);
    await open({ ...base, launcherImportScan: scan, pickFolder: async () => "D:\\Jeux" });
    expect(await screen.findByText(/Aucune instance trouvée/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Choisir un dossier/ }));
    expect(await screen.findByText("Choisie")).toBeTruthy();
    expect(scan).toHaveBeenLastCalledWith("D:\\Jeux");
  });

  it("sélecteur annulé : rien ne change ; sélecteur en panne : saisie du chemin à la main", async () => {
    const base = createFakeServices({ tickMs: 0, detectedInstances: [] });
    await open({ ...base, pickFolder: async () => null });
    fireEvent.click(await screen.findByRole("button", { name: /Choisir un dossier/ }));
    await waitFor(() => expect(screen.getByRole("button", { name: /Choisir un dossier/ })).toBeTruthy());
    expect(screen.queryByRole("alert")).toBeNull();
    cleanup();
    const svc = { ...base, pickFolder: () => Promise.reject(new Error("Pas de dialogue.")), launcherImportScan: vi.fn(base.launcherImportScan) };
    await open(svc);
    fireEvent.click(await screen.findByRole("button", { name: /Choisir un dossier/ }));
    expect((await screen.findByRole("alert")).textContent).toBe("Pas de dialogue.");
    fireEvent.change(screen.getByLabelText("Dossier d'une instance ou d'un lanceur"), { target: { value: "E:\\Prism" } });
    fireEvent.click(screen.getByRole("button", { name: "Chercher" }));
    await waitFor(() => expect(svc.launcherImportScan).toHaveBeenLastCalledWith("E:\\Prism"));
  });

  it("recherche impossible : l'erreur est dite et le dossier peut être choisi à la main", async () => {
    const base = createFakeServices({ tickMs: 0 });
    await open({ ...base, launcherImportScan: () => Promise.reject(new Error("recherche interrompue")) });
    expect((await screen.findByRole("alert")).textContent).toBe("recherche interrompue");
    expect(screen.getByLabelText("Dossier d'une instance ou d'un lanceur")).toBeTruthy();
  });

  it("« Autre méthode » revient au choix ; Échap retourne à l'accueil", async () => {
    await open(createFakeServices({ tickMs: 0 }));
    fireEvent.click(await screen.findByRole("button", { name: "Autre méthode" }));
    expect(await screen.findByRole("heading", { name: "Créer une instance" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Importer depuis un autre lanceur/ }));
    await screen.findByRole("list", { name: "Instances trouvées" });
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(window.location.hash).toBe("#/main");
  });
});
