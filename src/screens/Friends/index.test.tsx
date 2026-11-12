import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";

afterEach(() => { cleanup(); window.location.hash = ""; });

describe("Amis : onglets", () => {
  it("pattern WAI-ARIA : flèches, Home, End, tabIndex itinérant", () => {
    window.location.hash = "#/friends";
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    const [t1, t2] = screen.getAllByRole("tab");
    expect(screen.getAllByRole("tab").map((t) => t.textContent?.replace(/\d+$/, "").trim())).toEqual(["Tes amis", "Demandes"]);
    expect(t1.getAttribute("tabindex")).toBe("0");
    expect(t2.getAttribute("tabindex")).toBe("-1");
    t1.focus();
    fireEvent.keyDown(t1, { key: "ArrowRight" });
    expect(t2.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(t2);
    expect(t2.getAttribute("tabindex")).toBe("0");
    fireEvent.keyDown(t2, { key: "Home" });
    expect(document.activeElement).toBe(t1);
    fireEvent.keyDown(t1, { key: "End" });
    expect(document.activeElement).toBe(t2);
  });

  it("pas de bouton Retour en double (la barre de titre porte le retour) ; « Ajouter un ami » met le focus sur le champ", () => {
    window.location.hash = "#/friends";
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(screen.queryByRole("button", { name: "Retour à l'accueil" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Ajouter un ami" }));
    expect(document.activeElement).toBe(screen.getByLabelText("Code ami de ton ami"));
  });

  it("Échap revient à l'accueil, sauf dans un champ", () => {
    window.location.hash = "#/friends";
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    const input = screen.getAllByRole("textbox")[0];
    fireEvent.keyDown(input, { key: "Escape" });
    expect(window.location.hash).toBe("#/friends");
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(window.location.hash).toContain("#/main");
  });
});
