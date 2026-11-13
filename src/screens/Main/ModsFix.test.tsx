import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createFakeServices } from "../../services/fake";
import type { FixPlan, FixResult, ModCheck, ModIssue, Services } from "../../services";
import { ModsPanel } from "./ModsPanel";

afterEach(cleanup);

const errIssue: ModIssue = { file: "iris-1.2.0.jar", mod_name: "Iris", kind: "missing_dep", severity: "error", message: "Il manque Sodium." };
const wayIssue: ModIssue = { file: "waystones-1.6.0.jar", mod_name: "Waystones", kind: "wrong_minecraft", severity: "error", message: "Mauvaise version." };
const incompat: ModIssue = { file: "sodium-1.1.0.jar", mod_name: "Sodium", kind: "incompatible", severity: "error", message: "Incompatible avec Xaero.", related: "xaero-minimap-1.5.0.jar" };
const clean: ModCheck = { issues: [], checked: 8 };

const PLAN: FixPlan = {
  plan_id: "p1",
  actions: [
    { id: "a1", kind: "install", title: "Installer Sodium 0.6.0 (Modrinth)", detail: "Iris en a besoin.", source: "modrinth", issue_files: ["iris-1.2.0.jar"] },
    { id: "a2", kind: "update", title: "Mettre à jour Waystones vers 1.7.0 (CurseForge)", source: "curseforge", issue_files: ["waystones-1.6.0.jar"] },
    { id: "a3", kind: "enable", title: "Activer Lithium", issue_files: ["iris-1.2.0.jar"] },
  ],
  unresolved: [
    { file: "create-1.0.0.jar", mod_name: "Create", message: "Trop vieux.", reason: "Non distribuable.", page_url: "https://example.org/create" },
  ],
};

function setup(over: Partial<Services> = {}, check: ModCheck = { checked: 8, issues: [errIssue, wayIssue] }) {
  const services = {
    ...createFakeServices({ tickMs: 0 }),
    modsCheck: vi.fn(async () => check),
    modsFixPlan: vi.fn(async () => PLAN),
    modsFixApply: vi.fn(async (): Promise<FixResult> => ({ applied: ["a1", "a3"], failed: [], backup_id: "b1", check: clean })),
    modsFixUndo: vi.fn(async () => check),
    ...over,
  } as Services;
  render(<ModsPanel services={services} instanceId="inst-aeronautics" onCount={() => {}} onBrowse={() => {}} />);
  return services;
}

describe("Correction automatique", () => {
  it("bouton visible seulement avec une erreur", async () => {
    setup({}, { checked: 8, issues: [{ ...errIssue, severity: "warning" }] });
    await screen.findByText("iris-1.2.0.jar");
    expect(screen.queryByRole("button", { name: "Corriger automatiquement" })).toBeNull();
    cleanup();
    setup();
    expect(await screen.findByRole("button", { name: "Corriger automatiquement" })).toBeTruthy();
  });

  it("plan : modale avec actions cochées et non-résolus ; décocher envoie les bons ids", async () => {
    const s = setup();
    fireEvent.click(await screen.findByRole("button", { name: "Corriger automatiquement" }));
    const dlg = await screen.findByRole("dialog", { name: "Corrections proposées" });
    await within(dlg).findByRole("button", { name: "Appliquer (3)" });
    expect(s.modsFixPlan).toHaveBeenCalledWith("inst-aeronautics");
    const boxes = within(dlg).getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes.map((b) => b.checked)).toEqual([true, true, true]);
    expect(within(dlg).getByText("Modrinth")).toBeTruthy();
    expect(within(dlg).getByText("CurseForge")).toBeTruthy();
    expect(within(dlg).getByText("Local")).toBeTruthy();
    expect(within(dlg).getByText("À régler toi-même")).toBeTruthy();
    const link = within(dlg).getByRole("link", { name: "Ouvrir la page" });
    expect(link.getAttribute("target")).toBe("_blank");
    expect(document.activeElement).toBe(within(dlg).getByRole("button", { name: "Appliquer (3)" }));
    fireEvent.click(within(dlg).getByRole("checkbox", { name: /Mettre à jour Waystones/ }));
    fireEvent.click(within(dlg).getByRole("button", { name: "Appliquer (2)" }));
    await waitFor(() => expect(s.modsFixApply).toHaveBeenCalledWith("inst-aeronautics", "p1", ["a1", "a3"]));
  });

  it("résultat : bandeau, puis « Annuler la correction » appelle modsFixUndo", async () => {
    const s = setup();
    fireEvent.click(await screen.findByRole("button", { name: "Corriger automatiquement" }));
    fireEvent.click(await screen.findByRole("button", { name: "Appliquer (3)" }));
    expect(await screen.findByText("2 corrections appliquées.")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByText(/problème/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Annuler la correction" }));
    await waitFor(() => expect(s.modsFixUndo).toHaveBeenCalledWith("inst-aeronautics", "b1"));
    await waitFor(() => expect(screen.queryByText("2 corrections appliquées.")).toBeNull());
    expect(await screen.findByRole("button", { name: /2 problèmes de compatibilité/ })).toBeTruthy();
  });

  it("échec partiel affiché dans le bandeau", async () => {
    setup({ modsFixApply: async () => ({ applied: ["a1"], failed: [{ title: "Mettre à jour Waystones", error: "Hors ligne." }], backup_id: "b1", check: clean }) });
    fireEvent.click(await screen.findByRole("button", { name: "Corriger automatiquement" }));
    fireEvent.click(await screen.findByRole("button", { name: "Appliquer (3)" }));
    expect(await screen.findByText("1 correction appliquée.")).toBeTruthy();
    expect(screen.getByText("1 a échoué : Mettre à jour Waystones — Hors ligne.")).toBeTruthy();
  });

  it("échec du plan : message et Réessayer", async () => {
    let n = 0;
    const s = setup({ modsFixPlan: vi.fn(async () => { if (n++ === 0) throw new Error("CurseForge indisponible."); return PLAN; }) });
    fireEvent.click(await screen.findByRole("button", { name: "Corriger automatiquement" }));
    expect((await screen.findByRole("alert")).textContent).toBe("CurseForge indisponible.");
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(await screen.findByRole("button", { name: "Appliquer (3)" })).toBeTruthy();
    expect(s.modsFixPlan).toHaveBeenCalledTimes(2);
  });

  it("aucune action : message et section à régler soi-même ; Appliquer désactivé", async () => {
    setup({ modsFixPlan: async () => ({ ...PLAN, actions: [] }) });
    fireEvent.click(await screen.findByRole("button", { name: "Corriger automatiquement" }));
    expect(await screen.findByText("Aucune correction automatique trouvée.")).toBeTruthy();
    expect(screen.getByText("À régler toi-même")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Appliquer (0)" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("« Corriger » d'un seul problème : seules les actions liées sont cochées", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: /2 problèmes de compatibilité/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Corriger Waystones" }));
    const dlg = await screen.findByRole("dialog", { name: "Corrections proposées" });
    await within(dlg).findByRole("button", { name: "Appliquer (1)" });
    const boxes = within(dlg).getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes.map((b) => b.checked)).toEqual([false, true, false]);
    expect(within(dlg).queryByText("À régler toi-même")).toBeNull();
  });

  it("incompatibilité : « Désactiver X » appelle modsToggle", async () => {
    const modsToggle = vi.fn(async () => {});
    setup({ modsToggle }, { checked: 8, issues: [incompat] });
    fireEvent.click(await screen.findByRole("button", { name: /1 problème de compatibilité/ }));
    expect(screen.queryByRole("button", { name: /^Corriger Sodium/ })).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: "Désactiver xaero-minimap-1.5.0.jar" }));
    await waitFor(() => expect(modsToggle).toHaveBeenCalledWith("inst-aeronautics", "xaero-minimap-1.5.0.jar", false));
    fireEvent.click(screen.getByRole("button", { name: "Désactiver Sodium" }));
    await waitFor(() => expect(modsToggle).toHaveBeenCalledWith("inst-aeronautics", "sodium-1.1.0.jar", false));
  });

  it("autoFix ouvre la prévisualisation une fois", async () => {
    const onAuto = vi.fn();
    const services = {
      ...createFakeServices({ tickMs: 0 }),
      modsCheck: async () => clean,
      modsFixPlan: vi.fn(async () => PLAN),
    } as Services;
    const el = (
      <ModsPanel services={services} instanceId="inst-aeronautics" onCount={() => {}} onBrowse={() => {}} autoFix onAutoFixed={onAuto} />
    );
    const { rerender } = render(el);
    await screen.findByRole("dialog", { name: "Corrections proposées" });
    expect(onAuto).toHaveBeenCalledTimes(1);
    await act(async () => { rerender(el); });
    expect(services.modsFixPlan).toHaveBeenCalledTimes(1);
  });
});
