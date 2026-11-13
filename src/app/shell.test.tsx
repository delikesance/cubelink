import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Shell, useScreenTitle } from "./shell";
import { buildHash, navigate, parseHash, goBack, canGoBack, resetHistory } from "./router";
import { ShellContext, type ShellContextValue } from "./context";
import { createFakeServices } from "../services/fake";

function go(hash: string) {
  act(() => {
    window.location.hash = hash;
    window.dispatchEvent(new HashChangeEvent("hashchange"));
  });
}

beforeEach(() => { window.location.hash = ""; resetHistory(); });
afterEach(cleanup);

describe("routeur", () => {
  it("route par défaut : login", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(screen.getByRole("main").getAttribute("data-route")).toBe("login");
    expect(document.querySelector(".titlebar-title")!.textContent).toBe("Connexion");
  });

  it("navigation par hash et focus sur le contenu", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    go("#/legal");
    const main = screen.getByRole("main");
    expect(main.getAttribute("data-route")).toBe("legal");
    // le focus est dans le contenu : sur le h1 de l'écran s'il en a un (il le prend lui-même), sinon sur <main> (shell)
    const h1 = main.querySelector("h1");
    expect(document.activeElement).toBe(h1 ?? main);
    expect(document.querySelector(".titlebar-title")!.textContent).toBe("Mentions légales");
  });

  it("l'ancien écran Amis ouvre Paramètres > Amis", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    go("#/friends");
    expect(screen.getByRole("main").getAttribute("data-route")).toBe("settings");
    expect(screen.getByRole("tab", { name: "Amis" }).getAttribute("aria-selected")).toBe("true");
  });

  it("route inconnue : repli sur login", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    go("#/nimporte-quoi");
    expect(screen.getByRole("main").getAttribute("data-route")).toBe("login");
  });

  it("parse et construit les paramètres", () => {
    expect(parseHash("#/install?friend=abc")).toEqual({ route: "install", params: { friend: "abc" } });
    expect(buildHash("join", { friend: "x" })).toBe("#/join?friend=x");
  });

  it("lien d'évitement : donne le focus au contenu", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    fireEvent.click(screen.getByText("Aller au contenu"));
    expect(document.activeElement).toBe(screen.getByRole("main"));
  });
});

describe("titre contextuel", () => {
  it("modifiable par un écran, remis à zéro au démontage", () => {
    const calls: (string | null)[] = [];
    function Probe() { useScreenTitle("Monde de Alex"); return null; }
    const value: ShellContextValue = {
      services: createFakeServices({ tickMs: 0 }), route: "main", params: {},
      navigate, goBack, canGoBack: false, setTitle: (t) => { calls.push(t); },
    };
    const { unmount } = render(<ShellContext.Provider value={value}><Probe /></ShellContext.Provider>);
    expect(calls).toEqual(["Monde de Alex"]);
    unmount();
    expect(calls).toEqual(["Monde de Alex", null]);
  });
});

describe("barre de titre", () => {
  it("boutons Réduire / Fermer présents et sans effet hors Tauri", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    const min = screen.getByRole("button", { name: "Réduire" });
    const close = screen.getByRole("button", { name: "Fermer" });
    expect(() => { fireEvent.click(min); fireEvent.click(close); }).not.toThrow();
  });
});
