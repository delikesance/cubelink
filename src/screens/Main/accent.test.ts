import { describe, expect, it } from "vitest";
import { avatarColors, contrastRatio, ensureContrast, hashHue, INK_DARK, pickInk } from "./accent";
import { coverUpscale, SOFT_UPSCALE } from "./ArtParts";

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

describe("décor : agrandissement en cover", () => {
  it("une capture 992 px sur un écran 1844 px est floutée, une 2560 px ne l'est pas", () => {
    expect(coverUpscale({ w: 992, h: 733 }, { w: 1844, h: 996 })).toBeGreaterThan(SOFT_UPSCALE);
    expect(coverUpscale({ w: 2560, h: 1369 }, { w: 1844, h: 996 })).toBeLessThan(1);
    expect(coverUpscale({ w: 1920, h: 1080 }, { w: 1280, h: 720 }, 1.5)).toBe(1);
    expect(coverUpscale({ w: 0, h: 0 }, { w: 100, h: 100 })).toBe(1);
  });
});
