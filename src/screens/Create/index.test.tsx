import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services/types";
import { validateName } from "./index";

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
  await screen.findByRole("option", { name: "1.21.1 (recommandée)" });
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
  it("propose les trois chemins", async () => {
    await chooser(createFakeServices({ tickMs: 0 }));
    expect(screen.getByRole("button", { name: /Parcourir les modpacks/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Créer une instance vide/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Importer un fichier/ })).toBeTruthy();
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
    expect((screen.getByLabelText("Version de Minecraft") as HTMLSelectElement).value).toBe("1.21.1");
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
      name: "Mon monde", minecraft: "1.21.1", loader: { loader: "fabric", version: "" }, ram_mb: 4096,
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
    fireEvent.change(screen.getByLabelText("Mémoire allouée"), { target: { value: "8" } });
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
