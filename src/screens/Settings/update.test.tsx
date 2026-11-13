import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";

beforeEach(() => { window.location.hash = "#/settings"; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const open = () => fireEvent.click(screen.getByRole("tab", { name: "À propos" }));

describe("section Mise à jour", () => {
  it("affiche la version, vérifie puis indique que Cubelink est à jour", async () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    open();
    expect(await screen.findByText(/Version installée\s*:\s*0\.1\.0/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Chercher une mise à jour" }));
    expect(await screen.findByText("Cubelink est à jour.")).toBeTruthy();
  });

  it("téléchargement : progression et bouton désactivé, puis prête et installation", async () => {
    const fake = createFakeServices({ tickMs: 0 });
    render(<Shell services={fake} />);
    open();
    await screen.findByText(/Version installée\s*:\s*0\.1\.0/);
    act(() => fake.emitUpdateState({ phase: "downloading", version: "0.2.0", downloaded: 50, total: 100 }));
    expect(screen.getByText("Version 0.2.0 trouvée, téléchargement…")).toBeTruthy();
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("50");
    expect(screen.queryByRole("button", { name: "Chercher une mise à jour" })).toBeNull();
    act(() => fake.emitUpdateState({ phase: "ready" }));
    expect(screen.getByText("Version 0.2.0 prête à installer.")).toBeTruthy();
    // Version prête : un seul bouton, celui qui installe.
    expect(screen.queryByRole("button", { name: "Chercher une mise à jour" })).toBeNull();
    const spy = vi.spyOn(fake, "updateInstall");
    fireEvent.click(screen.getAllByRole("button", { name: "Redémarrer et installer" })[0]);
    expect(spy).toHaveBeenCalled();
  });

  it("affiche l'erreur", async () => {
    const fake = createFakeServices({ tickMs: 0 });
    render(<Shell services={fake} />);
    open();
    await screen.findByText(/Version installée\s*:\s*0\.1\.0/);
    act(() => fake.emitUpdateState({ phase: "error", message: "Serveur injoignable." }));
    expect(screen.getByText("Serveur injoignable.")).toBeTruthy();
  });
});

describe("bannière de mise à jour", () => {
  it("invisible hors phase ready ; visible ensuite, installe au clic, fermable", async () => {
    const fake = createFakeServices({ tickMs: 0 });
    render(<Shell services={fake} />);
    await waitFor(() => expect(screen.getByRole("main")).toBeTruthy());
    expect(screen.queryByRole("region", { name: "Mise à jour" })).toBeNull();
    act(() => fake.emitUpdateState({ phase: "downloading", version: "0.2.0" }));
    expect(screen.queryByRole("region", { name: "Mise à jour" })).toBeNull();
    act(() => fake.emitUpdateState({ phase: "ready" }));
    expect(screen.getByRole("region", { name: "Mise à jour" }).textContent).toMatch(/0\.2\.0 prête/);
    const spy = vi.spyOn(fake, "updateInstall");
    fireEvent.click(screen.getByRole("button", { name: "Redémarrer et installer" }));
    expect(spy).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Plus tard" }));
    expect(screen.queryByRole("region", { name: "Mise à jour" })).toBeNull();
  });
});
