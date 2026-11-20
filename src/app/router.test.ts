import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { goBack, navigate, parseHash, resetHistory, ROUTES, setBackGuard } from "./router";
import { ROUTE_LABELS } from "./shell";

describe("routes", () => {
  it("contient browse, avec libellé et paramètres", () => {
    expect(ROUTES).toContain("browse");
    expect(ROUTE_LABELS.browse).toBe("Parcourir");
    expect(parseHash("#/browse?kind=mod&instance=i1")).toEqual({ route: "browse", params: { kind: "mod", instance: "i1" } });
  });
});

describe("garde de retour", () => {
  const go = (to: Parameters<typeof navigate>[0]) => { navigate(to); window.dispatchEvent(new HashChangeEvent("hashchange")); };
  beforeEach(() => { window.location.hash = "#/login"; resetHistory(); go("main"); go("join"); });
  afterEach(() => resetHistory());

  it("une garde qui renvoie true bloque goBack, sinon le retour passe", () => {
    const guard = vi.fn(() => true);
    const off = setBackGuard(guard);
    goBack();
    expect(guard).toHaveBeenCalledTimes(1);
    expect(window.location.hash).toBe("#/join");
    off();
    goBack();
    expect(guard).toHaveBeenCalledTimes(1);
    expect(window.location.hash).toBe("#/main");
  });

  it("bloque aussi le retour de secours (pas d'historique) et ne touche pas à navigate", () => {
    window.location.hash = "#/join"; resetHistory();
    setBackGuard(() => true);
    goBack("settings");
    expect(window.location.hash).toBe("#/join");
    navigate("settings");
    expect(window.location.hash).toBe("#/settings");
  });

  it("le désenregistreur ne retire que sa propre garde", () => {
    const offA = setBackGuard(() => true);
    const offB = setBackGuard(() => true);
    offA();
    goBack();
    expect(window.location.hash).toBe("#/join");
    offB();
    goBack();
    expect(window.location.hash).toBe("#/main");
  });

  it("resetHistory efface la garde", () => {
    setBackGuard(() => true);
    resetHistory();
    goBack("settings");
    expect(window.location.hash).not.toBe("#/join");
  });
});
