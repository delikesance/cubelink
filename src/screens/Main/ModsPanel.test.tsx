import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { createFakeServices } from "../../services/fake";
import type { ModCheck, Services } from "../../services";
import { ModsPanel } from "./ModsPanel";

afterEach(cleanup);

const setup = (check: ModCheck | Error) => {
  const services: Services = {
    ...createFakeServices({ tickMs: 0 }),
    modsCheck: async () => { if (check instanceof Error) throw check; return check; },
  };
  render(<ModsPanel services={services} instanceId="inst-aeronautics" onCount={() => {}} onBrowse={() => {}} />);
};

describe("ModsPanel compatibilité", () => {
  it("affiche le bandeau, la liste dépliable et la pastille sur la ligne", async () => {
    setup({ checked: 8, issues: [
      { file: "iris-1.2.0.jar", mod_name: "Iris", kind: "missing_dep", severity: "error", message: "Il manque Sodium." },
      { file: "create-1.0.0.jar", mod_name: "Create", kind: "incompatible", severity: "warning", message: "Gêne Sodium.", related: "sodium-1.1.0.jar" },
    ] });
    const head = await screen.findByRole("button", { name: /2 problèmes de compatibilité/ });
    expect(head.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(head);
    expect(head.getAttribute("aria-expanded")).toBe("true");
    const list = screen.getByRole("list", { name: "Problèmes de compatibilité" });
    expect(list.textContent).toContain("Iris : Il manque Sodium.");
    const row = (await screen.findByText("iris-1.2.0.jar")).closest("li")!;
    expect(within(row).getByText("Il manque Sodium.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Voir sodium-1.1.0.jar" }));
    expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe("sodium-1.1.0.jar");
  });

  it("rien quand la liste est vide ou si l'appel échoue", async () => {
    setup({ checked: 8, issues: [] });
    await screen.findByText("iris-1.2.0.jar");
    expect(screen.queryByText(/problème/)).toBeNull();
    cleanup();
    setup(new Error("absent"));
    await screen.findByText("iris-1.2.0.jar");
    expect(screen.queryByText(/problème/)).toBeNull();
  });
});
