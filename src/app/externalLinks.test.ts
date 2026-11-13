import { describe, expect, it } from "vitest";
import { isExternalHref } from "./externalLinks";

describe("liens externes", () => {
  const origin = "http://tauri.localhost";
  it("un lien interne de l'appli (hash) reste dans l'appli", () => {
    expect(isExternalHref("http://tauri.localhost/#/legal", origin)).toBe(false);
    expect(isExternalHref("http://tauri.localhost/#/settings?tab=apropos", origin)).toBe(false);
  });
  it("un site web part dans le navigateur", () => {
    expect(isExternalHref("https://opengameart.org/content/oga-by-30-faq", origin)).toBe(true);
    expect(isExternalHref("https://www.minecraft.net/fr-fr/eula", origin)).toBe(true);
  });
  it("autres schémas ignorés", () => {
    expect(isExternalHref("mailto:a@b.c", origin)).toBe(false);
  });
});
