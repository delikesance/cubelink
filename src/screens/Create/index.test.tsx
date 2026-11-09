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

async function ready(svc: Services) {
  render(<Shell services={svc} />);
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

describe("écran Créer une instance", () => {
  it("dialogue accessible, 1.21.1 sélectionnée, NeoForge par défaut", async () => {
    await ready(createFakeServices({ tickMs: 0 }));
    const dlg = screen.getByRole("dialog", { name: "Créer une instance" });
    expect(dlg.getAttribute("aria-modal")).toBe("true");
    expect((screen.getByLabelText("Version de Minecraft") as HTMLSelectElement).value).toBe("1.21.1");
    expect((screen.getByRole("radio", { name: "NeoForge" }) as HTMLInputElement).checked).toBe(true);
  });

  it("création réussie : instancesCreate puis retour à main", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const create = vi.fn(base.instancesCreate);
    await ready({ ...base, instancesCreate: create });
    fireEvent.change(nameInput(), { target: { value: "Mon monde" } });
    fireEvent.click(screen.getByRole("radio", { name: "Fabric" }));
    fireEvent.click(createBtn());
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
    expect(create).toHaveBeenCalledWith({
      name: "Mon monde", minecraft: "1.21.1", loader: { loader: "fabric", version: "" }, ram_mb: 4096,
    });
  });

  it("nom invalide : message lié au champ, aucun appel", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const create = vi.fn(base.instancesCreate);
    await ready({ ...base, instancesCreate: create });
    fireEvent.click(createBtn());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/obligatoire/);
    expect(nameInput().getAttribute("aria-describedby")).toBe(alert.id);
    fireEvent.change(nameInput(), { target: { value: "a/b" } });
    fireEvent.click(createBtn());
    expect((await screen.findByRole("alert")).textContent).toMatch(/caractères/);
    expect(create).not.toHaveBeenCalled();
    expect(window.location.hash).toBe("#/create");
  });

  it("Échap annule et revient à main", async () => {
    await ready(createFakeServices({ tickMs: 0 }));
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(window.location.hash).toBe("#/main");
  });

  it("Annuler revient à main", async () => {
    await ready(createFakeServices({ tickMs: 0 }));
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(window.location.hash).toBe("#/main");
  });

  it("erreur du service affichée, écran conservé", async () => {
    const base = createFakeServices({ tickMs: 0 });
    await ready({ ...base, instancesCreate: () => Promise.reject(new Error("Disque plein.")) });
    fireEvent.change(nameInput(), { target: { value: "Test" } });
    fireEvent.click(createBtn());
    expect((await screen.findByRole("alert")).textContent).toContain("Disque plein.");
    expect(window.location.hash).toBe("#/create");
    expect((createBtn() as HTMLButtonElement).disabled).toBe(false);
  });

  it("import .mrpack par chemin saisi", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const imp = vi.fn(base.instancesImportMrpack);
    await ready({ ...base, instancesImportMrpack: imp });
    fireEvent.click(screen.getByRole("button", { name: /Importer un modpack/ }));
    fireEvent.change(screen.getByLabelText("Chemin du fichier .mrpack"), { target: { value: "C:\\p\\a.mrpack" } });
    fireEvent.click(screen.getByRole("button", { name: "Importer" }));
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
    expect(imp).toHaveBeenCalledWith("C:\\p\\a.mrpack");
  });

  it("le champ Nom est focalisé à l'ouverture (le shell ne vole pas le focus) et Échap ferme depuis body", async () => {
    await ready(createFakeServices({ tickMs: 0 }));
    expect(document.activeElement).toBe(nameInput());
    (document.activeElement as HTMLElement).blur();
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(window.location.hash).toBe("#/main");
  });
});
