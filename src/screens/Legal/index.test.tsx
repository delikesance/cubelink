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
    expect(screen.getByRole("heading", { level: 2, name: "Mentions légales et confidentialité" })).toBeTruthy();
    for (const s of SECTIONS) expect(screen.getByRole("heading", { level: 3, name: new RegExp(s.label === "Licences" ? "Licences" : s.label.split(" ")[0]!) })).toBeTruthy();
    expect(screen.getByText(/n'est pas approuvé par Mojang ou Microsoft/)).toBeTruthy();
  });

  it("la navigation des réglages renvoie aux onglets, À propos est la page courante", () => {
    setup();
    const nav = screen.getByRole("navigation", { name: "Sections des réglages" });
    expect(nav.querySelectorAll("a")).toHaveLength(5);
    expect(screen.getByRole("link", { name: "À propos" }).getAttribute("aria-current")).toBe("page");
    expect(screen.getByRole("link", { name: "Java" }).getAttribute("href")).toBe("#/settings?tab=java");
  });

  it("Échap revient via goBack(settings)", () => {
    const navigate = setup();
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(navigate.goBack).toHaveBeenCalledWith("settings");
  });
});
