import { describe, expect, it } from "vitest";
import { lastSeenText } from "./lastSeen";

describe("lastSeenText", () => {
  it("dernière connexion : relative, puis date", () => {
    const now = Date.UTC(2026, 10, 12, 12) / 1000;
    expect(lastSeenText(null, now)).toBeNull();
    expect(lastSeenText(now - 20, now)).toBe("Vu à l'instant");
    expect(lastSeenText(now - 5 * 60, now)).toBe("Vu il y a 5 min");
    expect(lastSeenText(now - 3 * 3600, now)).toBe("Vu il y a 3 h");
    expect(lastSeenText(now - 30 * 3600, now)).toBe("Vu hier");
    expect(lastSeenText(now - 4 * 86400, now)).toBe("Vu il y a 4 jours");
    expect(lastSeenText(Date.UTC(2026, 6, 3, 12) / 1000, now)).toBe("Vu le 3 juillet");
    expect(lastSeenText(Date.UTC(2025, 6, 3, 12) / 1000, now)).toBe("Vu le 3 juillet 2025");
  });
});
