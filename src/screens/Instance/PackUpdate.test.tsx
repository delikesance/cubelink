import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createFakeServices } from "../../services/fake";
import type { HearthCard, Instance, ModpackUpdate } from "../../services";
import { PackUpdate, summaryOf } from "./PackUpdate";

afterEach(cleanup);

const UPDATE: ModpackUpdate = { version_id: "v2", name: "Aeronautics 1.4.2", version_number: "1.4.2", published: "2026-04-01T00:00:00Z" };

function setup(opts: { update?: ModpackUpdate | null | "offline"; locked?: boolean; source?: string; hearth?: Partial<HearthCard> | null } = {}) {
  const services = createFakeServices({ tickMs: 0, requestCount: 0 });
  const inst = { id: "inst-aeronautics", pack: { source: opts.source ?? "modrinth", project_id: "P", version_id: "v1" } } as Instance;
  services.setPackUpdate(inst.id, opts.update === undefined ? UPDATE : opts.update);
  if (opts.hearth) services.seedHearth({ id: "g1", name: "Les copains", instance_id: inst.id, ...opts.hearth });
  const onChanged = vi.fn();
  render(<PackUpdate services={services} inst={inst} locked={opts.locked ?? false} onChanged={onChanged} />);
  return { services, onChanged };
}

describe("PackUpdate", () => {
  it("summaryOf omet les compteurs à zéro", () => {
    expect(summaryOf({ added: 3, removed: 1, replaced: 12, kept: [], backup_id: "b" })).toBe("3 fichiers ajoutés · 1 retiré · 12 remplacés");
    expect(summaryOf({ added: 0, removed: 0, replaced: 1, kept: [], backup_id: "b" })).toBe("1 remplacé");
    expect(summaryOf({ added: 0, removed: 0, replaced: 0, kept: [], backup_id: "b" })).toBe("Aucun fichier à changer");
  });

  it("affiche le badge quand une mise à jour est disponible", async () => {
    setup();
    expect(await screen.findByText("Mise à jour 1.4.2 disponible")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Mettre à jour" })).toBeTruthy();
  });

  it("rien à afficher si à jour, hors ligne ou si le pack ne vient pas de Modrinth", async () => {
    for (const o of [{ update: null }, { update: "offline" as const }, { source: "curseforge" }]) {
      const { services } = setup(o);
      await Promise.resolve();
      await waitFor(() => expect(screen.queryByRole("region", { name: "Mise à jour du modpack" })).toBeNull());
      expect(services.packUpdateCalls).toEqual([]);
      cleanup();
    }
  });

  it("jeu en cours : bouton désactivé avec explication", async () => {
    setup({ locked: true });
    const b = await screen.findByRole("button", { name: "Mettre à jour" });
    expect((b as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Ferme le jeu pour mettre à jour.")).toBeTruthy();
  });

  it("Mettre à jour : résumé, fichiers du joueur gardés, puis Annuler appelle modpackUpdateUndo", async () => {
    const { services, onChanged } = setup();
    fireEvent.click(await screen.findByRole("button", { name: "Mettre à jour" }));
    expect(await screen.findByText("Modpack mis à jour")).toBeTruthy();
    expect(services.packUpdateCalls).toEqual([{ kind: "apply", args: ["inst-aeronautics", "v2"] }]);
    expect(screen.getByText("3 fichiers ajoutés · 1 retiré · 12 remplacés.")).toBeTruthy();
    expect(screen.getByText(/1 fichier à toi a été conservé tel quel : config\/sodium-options\.json/)).toBeTruthy();
    expect(onChanged).toHaveBeenCalledTimes(1);
    services.setPackUpdate("inst-aeronautics", UPDATE);
    fireEvent.click(screen.getByRole("button", { name: "Annuler la mise à jour" }));
    await waitFor(() => expect(screen.getByText(/Mise à jour annulée/)).toBeTruthy());
    expect(services.packUpdateCalls[1]).toEqual({ kind: "undo", args: ["inst-aeronautics", "pack-backup-demo"] });
    expect(onChanged).toHaveBeenCalledTimes(2);
    // La proposition réapparaît après l'annulation.
    expect(await screen.findByText("Mise à jour 1.4.2 disponible")).toBeTruthy();
  });

  it("instance liée à un groupe Hearth : propose la mise à jour au groupe", async () => {
    const { services } = setup({ hearth: {} });
    fireEvent.click(await screen.findByRole("button", { name: "Mettre à jour" }));
    fireEvent.click(await screen.findByRole("button", { name: "Proposer la mise à jour au groupe" }));
    expect(await screen.findByText(/Proposition envoyée au groupe « Les copains »/)).toBeTruthy();
    expect(services.hearthCalls.find((c) => c.kind === "PackUpdate")?.args).toEqual(["g1", "inst-aeronautics"]);
  });

  it("sans groupe : pas de proposition Hearth", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "Mettre à jour" }));
    await screen.findByText("Modpack mis à jour");
    expect(screen.queryByRole("button", { name: "Proposer la mise à jour au groupe" })).toBeNull();
  });

  it("échec de la mise à jour : message d'erreur, le bouton reste disponible", async () => {
    const { services } = setup();
    services.modpackUpdateApply = async () => { throw new Error("réseau : coupé"); };
    fireEvent.click(await screen.findByRole("button", { name: "Mettre à jour" }));
    expect((await screen.findByRole("alert")).textContent).toContain("coupé");
    expect(screen.getByRole("button", { name: "Mettre à jour" })).toBeTruthy();
  });
});
