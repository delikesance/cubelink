import { describe, expect, it } from "vitest";
import { checkedAgo } from "./checkedAgo";

describe("checkedAgo", () => {
  const now = 1_000_000;
  it("formule l'âge du dernier contrôle", () => {
    expect(checkedAgo(now - 5, now)).toBe("Vérifié à l'instant");
    expect(checkedAgo(now - 5 * 60, now)).toBe("Vérifié il y a 5 min");
    expect(checkedAgo(now - 2 * 3600 - 10, now)).toBe("Vérifié il y a 2 h");
    expect(checkedAgo(now - 86400, now)).toBe("Vérifié hier");
    expect(checkedAgo(now - 3 * 86400, now)).toBe("Vérifié il y a 3 jours");
  });
  it("horloge en retard : jamais d'âge négatif", () => {
    expect(checkedAgo(now + 500, now)).toBe("Vérifié à l'instant");
  });
});
