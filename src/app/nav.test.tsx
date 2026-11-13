import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Shell } from "./shell";
import { canGoBack, goBack, navigate, resetHistory } from "./router";
import { createFakeServices } from "../services/fake";

const hash = () => window.location.hash;
function nav(r: Parameters<typeof navigate>[0]) { act(() => { navigate(r); window.dispatchEvent(new HashChangeEvent("hashchange")); }); }
function back(fb?: Parameters<typeof goBack>[0]) { act(() => { goBack(fb); window.dispatchEvent(new HashChangeEvent("hashchange")); }); }

beforeEach(() => { window.location.hash = "#/login"; resetHistory(); });
afterEach(cleanup);

describe("historique", () => {
  it("pile, retour, et plus de retour possible au départ", () => {
    nav("main"); nav("create");
    expect(canGoBack()).toBe(true);
    back();
    expect(hash()).toBe("#/main");
    expect(canGoBack()).toBe(false); // login est sauté
  });

  it("pas de boucle A→B→A→B : revenir à la précédente fait un pop", () => {
    nav("main"); nav("browse"); nav("main"); nav("browse");
    back();
    expect(hash()).toBe("#/main");
    back("settings");
    expect(hash()).toBe("#/settings"); // plus d'historique exploitable : fallback
  });

  it("fallback par défaut : main", () => {
    window.location.hash = "#/browse"; resetHistory();
    back();
    expect(hash()).toBe("#/main");
  });

  it("login et error ne sont jamais des cibles", () => {
    nav("main"); nav("error"); nav("legal");
    back();
    expect(hash()).toBe("#/main");
  });
});

describe("TitleBar", () => {
  it("bouton Retour désactivé sur login, actif ensuite ; Agrandir présent", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    const b = screen.getByRole("button", { name: "Retour" }) as HTMLButtonElement;
    expect(b.disabled).toBe(true);
    nav("main"); nav("browse");
    expect(b.disabled).toBe(false);
    fireEvent.click(b);
    expect(hash()).toBe("#/main");
    expect(screen.getByRole("button", { name: "Agrandir" })).toBeTruthy();
  });
});

describe("raccourcis globaux", () => {
  it("Alt+← et souris 3 reviennent en arrière", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    nav("main"); nav("browse"); nav("legal");
    fireEvent.keyDown(window, { key: "ArrowLeft", altKey: true });
    expect(hash()).toBe("#/browse");
    fireEvent.mouseUp(window, { button: 3 });
    expect(hash()).toBe("#/main");
  });

  it("ignorés si dialogue modal ouvert ; Alt+← ignoré dans un champ", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    nav("main"); nav("browse");
    const input = document.createElement("input");
    document.body.appendChild(input); input.focus();
    fireEvent.keyDown(input, { key: "ArrowLeft", altKey: true });
    expect(hash()).toBe("#/browse");
    input.remove();
    const dlg = document.createElement("div");
    dlg.setAttribute("aria-modal", "true"); document.body.appendChild(dlg);
    fireEvent.mouseUp(window, { button: 3 });
    fireEvent.keyDown(window, { key: "ArrowLeft", altKey: true });
    expect(hash()).toBe("#/browse");
    dlg.remove();
  });
});
