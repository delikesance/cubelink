import { describe, expect, it } from "vitest";
import { parseLine, splitTag } from "./LogsPanel";

describe("lignes de log", () => {
  it("préfixe de mod mis à part, message intact", () => {
    expect(splitTag("[SDLNativesLoader] Loaded SDL from controlify")).toEqual(["[SDLNativesLoader] ", "Loaded SDL from controlify"]);
    expect(splitTag("[ETF]: reloading ETF data.")).toEqual(["[ETF]: ", "reloading ETF data."]);
    expect(splitTag("Loaded 9 Polytone config entries")).toEqual(["", "Loaded 9 Polytone config entries"]);
    // Un tableau en début de message n'est pas un préfixe.
    expect(splitTag("[_e] suffixes")).toEqual(["[_e] ", "suffixes"]);
    expect(splitTag("[ ] vide")).toEqual(["", "[ ] vide"]);
  });

  it("format Minecraft : heure, fil, niveau, message", () => {
    expect(parseLine("[13:15:42] [Render thread/WARN]: [SDLNativesLoader] mismatch")).toEqual({
      time: "13:15:42", level: "WARN", thread: "Render thread", text: "[SDLNativesLoader] mismatch",
    });
  });
});
