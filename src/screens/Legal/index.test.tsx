import { describe, expect, it, vi, afterEach } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import Legal from "./index";
import { SECTIONS } from "./sections";

afterEach(cleanup);

function setup() {
  const navigate = vi.fn();
  const goBack = vi.fn();
  const value = {
    services: {} as ShellContextValue["services"], route: "legal", params: {},
    navigate, goBack, canGoBack: false, setTitle: () => {},
  } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Legal /></ShellContext.Provider>);
  return Object.assign(navigate, { goBack });
}

describe("Legal", () => {
  it("affiche toutes les sections à la suite, sous le titre", () => {
    setup();
    expect(screen.getByRole("heading", { level: 1, name: "Mentions légales et confidentialité" })).toBeTruthy();
    for (const s of SECTIONS) expect(screen.getByRole("heading", { level: 3, name: s.label === "Licences" ? /^Licences/ : s.label })).toBeTruthy();
    expect(screen.getByText(/n'est pas approuvé par Mojang ou Microsoft/)).toBeTruthy();
    expect(screen.getByText(/sans but commercial/)).toBeTruthy();
    expect(screen.queryByText(/à compléter/)).toBeNull();
  });

  it("Retour revient à l'écran d'origine (Paramètres par défaut), dans l'appli", () => {
    const navigate = setup();
    expect(screen.queryByRole("navigation")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retour" }));
    expect(navigate.goBack).toHaveBeenCalledWith("settings");
  });

  it("Échap revient via goBack(settings)", () => {
    const navigate = setup();
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(navigate.goBack).toHaveBeenCalledWith("settings");
  });
});
