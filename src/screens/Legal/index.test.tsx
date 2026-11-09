import { describe, expect, it, vi, afterEach } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import Legal from "./index";

afterEach(cleanup);

function setup() {
  const navigate = vi.fn();
  const value = {
    services: {} as ShellContextValue["services"], route: "legal", params: {},
    navigate, setTitle: () => {},
  } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Legal /></ShellContext.Provider>);
  return navigate;
}

describe("Legal", () => {
  it("affiche une seule section à la fois", () => {
    setup();
    expect(screen.getAllByRole("tab")).toHaveLength(6);
    expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 2, name: "Éditeur" })).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "Non-affiliation" }));
    expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
    expect(screen.queryByRole("heading", { level: 2, name: "Éditeur" })).toBeNull();
    expect(screen.getByText(/n'est pas approuvé par Mojang ou Microsoft/)).toBeTruthy();
  });

  it("change de section aux flèches, avec Home/End et bouclage", () => {
    setup();
    const tab = (n: string) => screen.getByRole("tab", { name: n });
    tab("Éditeur").focus();
    fireEvent.keyDown(tab("Éditeur"), { key: "ArrowDown" });
    expect(tab("Non-affiliation").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tab("Non-affiliation"));
    expect(tab("Éditeur").getAttribute("tabindex")).toBe("-1");
    fireEvent.keyDown(tab("Non-affiliation"), { key: "End" });
    expect(tab("Contact").getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(tab("Contact"), { key: "ArrowDown" });
    expect(tab("Éditeur").getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(tab("Éditeur"), { key: "ArrowUp" });
    expect(tab("Contact").getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("heading", { level: 2, name: "Contact" })).toBeTruthy();
  });

  it("le bouton Retour renvoie aux paramètres, Fermer à l'accueil", () => {
    const navigate = setup();
    fireEvent.click(screen.getByRole("button", { name: "Retour aux paramètres" }));
    expect(navigate).toHaveBeenCalledWith("settings");
    fireEvent.click(screen.getByRole("button", { name: "Fermer" }));
    expect(navigate).toHaveBeenCalledWith("main");
  });
});
