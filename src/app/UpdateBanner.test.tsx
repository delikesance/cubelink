import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createFakeServices } from "../services/fake";
import { UpdateBanner } from "./UpdateBanner";

afterEach(cleanup);

describe("texte de la bannière de mise à jour", () => {
  it("tutoie le joueur et ne promet pas l'absence de perte", async () => {
    const fake = createFakeServices({ tickMs: 0 });
    render(<UpdateBanner services={fake} />);
    act(() => fake.emitUpdateState({ phase: "ready", version: "0.2.0" }));
    const txt = (await screen.findByRole("status")).textContent ?? "";
    expect(txt).toContain("Redémarre Cubelink pour l'installer");
    // update_install ne contrôle ni les serveurs Hearth ni les transferts : pas de promesse (#181).
    expect(txt).not.toMatch(/Rien n'est perdu/);
    expect(txt).not.toMatch(/\b(Redémarrez|Installez|Vérifiez|Réessayez)\b/);
  });
});
