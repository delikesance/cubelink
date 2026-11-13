import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
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
const grid = () => screen.getByRole("group", { name: "Instances" });
const AERO = "Create: Aeronautics";

const withArt = (art: Partial<NonNullable<Instance["art"]>>): Instance => ({
  id: "inst-aeronautics", name: AERO, minecraft: "1.21.1", loader: { loader: "neoforge", version: "21.1.77" }, ram_mb: 6144, mod_count: 94,
  art: { source: "modrinth", project_id: "p", title: "Pack", author: "Zed", summary: "Résumé trouvé", color: "#27ae60", icon: null, screenshots: [], page_url: null, ...art },
});

describe("Main : illustrations des tuiles", () => {
  it("image en erreur : la tuile reste avec les initiales", async () => {
    setup({ instancesArtImage: async () => { throw new Error("boom"); } });
    await screen.findByRole("group", { name: "Instances" });
    expect(within(grid()).getByRole("button", { name: "All the Mods 10" }).querySelector("img")).toBeNull();
  });

  it("recherche automatique : une seule fois par instance sans illustration, la tuile suit", async () => {
    const lookup = vi.fn(async (id: string) => { if (id === "inst-aeronautics") return withArt({}); throw new Error("Aucun modpack correspondant"); });
    setup({ instancesArtLookup: lookup });
    // Recherche silencieuse : mode automatique (nom identique, jamais une instance créée à la main).
    await waitFor(() => expect(lookup).toHaveBeenCalledWith("inst-aeronautics", true));
    const ids = lookup.mock.calls.map((c) => c[0]);
    expect(new Set(ids).size).toBe(ids.length);
    // l'instance mise à jour reste affichée, sans erreur visible
    expect(within(grid()).getByRole("button", { name: AERO })).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("recherche automatique : un échec reste silencieux", async () => {
    const lookup = vi.fn(async () => { throw new Error("hors-ligne"); });
    setup({ instancesArtLookup: lookup });
    await waitFor(() => expect(lookup).toHaveBeenCalled());
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByText("hors-ligne")).toBeNull();
  });

  it("instance avec illustration : pas de recherche automatique", async () => {
    const lookup = vi.fn(async () => withArt({}));
    setup({ instancesArtLookup: lookup });
    await screen.findByRole("button", { name: "All the Mods 10" });
    await waitFor(() => expect(lookup).toHaveBeenCalled());
    expect(lookup).not.toHaveBeenCalledWith("inst-atm10");
  });
});
