import { describe, expect, it } from "vitest";
import { parseHash, ROUTES } from "./router";
import { ROUTE_LABELS } from "./shell";

describe("routes", () => {
  it("contient browse, avec libellé et paramètres", () => {
    expect(ROUTES).toContain("browse");
    expect(ROUTE_LABELS.browse).toBe("Parcourir");
    expect(parseHash("#/browse?kind=mod&instance=i1")).toEqual({ route: "browse", params: { kind: "mod", instance: "i1" } });
  });
});
