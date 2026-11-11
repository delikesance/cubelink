import { describe, expect, it } from "vitest";
import { avatarColors, contrastRatio, ensureContrast, hashHue, INK_DARK, pickInk } from "./accent";

describe("contraste", () => {
  it("noir/blanc = 21, identique = 1", () => {
    expect(contrastRatio("#000000", "#FFFFFF")).toBeCloseTo(21, 0);
    expect(contrastRatio("#336699", "#336699")).toBeCloseTo(1, 5);
  });
  it("ensureContrast éclaircit ou assombrit jusqu'au seuil", () => {
    const dark = ensureContrast("#777777", "#FFFFFF", 4.5);
    expect(contrastRatio(dark, "#FFFFFF")).toBeGreaterThanOrEqual(4.5);
    const light = ensureContrast("#777777", INK_DARK, 4.5);
    expect(contrastRatio(light, INK_DARK)).toBeGreaterThanOrEqual(4.5);
  });
  it("pickInk choisit l'encre la plus lisible", () => {
    expect(pickInk("#FFFF00")).toBe(INK_DARK);
    expect(pickInk("#001133")).toBe("#FFFFFF");
  });
});

describe("avatarColors", () => {
  const colors = ["#ff0000", "#00ff00", "#0000ff", "#ffff00", "#808080", "#2f6fed", "#c0392b", "#ffffff", "#000000", "#777777", "#ff5c00"];
  it.each(colors)("garantit le contraste de l'initiale pour %s", (c) => {
    const t = avatarColors(c, "x");
    expect(contrastRatio(t.bg, t.ink)).toBeGreaterThanOrEqual(4.5);
  });
  it("repli stable par hash du nom quand la couleur manque ou est invalide", () => {
    expect(avatarColors(null, "Mon pack")).toEqual(avatarColors(undefined, "Mon pack"));
    expect(avatarColors("rouge", "Mon pack")).toEqual(avatarColors(null, "Mon pack"));
    expect(avatarColors(null, "Mon pack").bg).not.toBe(avatarColors(null, "Autre pack").bg);
    expect(hashHue("Mon pack")).toBe(hashHue("Mon pack"));
    const t = avatarColors(null, "Mon pack");
    expect(contrastRatio(t.bg, t.ink)).toBeGreaterThanOrEqual(4.5);
  });
});
