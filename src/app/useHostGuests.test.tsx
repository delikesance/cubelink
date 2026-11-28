import { afterEach, describe, expect, it } from "vitest";
import { cleanup, renderHook, waitFor } from "@testing-library/react";
import { createFakeServices } from "../services/fake";
import type { HostGuest } from "../services/types";
import { invitedLabel, pendingInviteKeys, remainingText, useHostGuests } from "./useHostGuests";

afterEach(cleanup);

const NOW = 1_000_000;
const guest = (over: Partial<HostGuest> & { public_key: string }): HostGuest => ({ origin: "invite", entered: false, expires_at: NOW + 120, ...over });

describe("pendingInviteKeys", () => {
  it("garde les invitations en attente, écarte échues, entrées et autres origines", () => {
    const keys = pendingInviteKeys([
      guest({ public_key: "a" }),
      guest({ public_key: "b", expires_at: NOW - 1 }),
      guest({ public_key: "c", entered: true, expires_at: null }),
      guest({ public_key: "d", origin: "request" }),
    ], NOW);
    expect([...keys]).toEqual(["a"]);
  });
});

describe("remainingText", () => {
  it("arrondit les minutes au-dessus, « < 1 min » sous 60 s", () => {
    expect(remainingText(NOW + 240, NOW)).toBe("4 min");
    expect(remainingText(NOW + 181, NOW)).toBe("4 min");
    expect(remainingText(NOW + 60, NOW)).toBe("1 min");
    expect(remainingText(NOW + 59, NOW)).toBe("< 1 min");
    expect(invitedLabel(NOW + 30, NOW)).toBe("Invité · < 1 min");
  });
});

describe("useHostGuests", () => {
  it("masque un invité échu, garde un entré sans échéance, suit l'événement", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const now = Math.floor(Date.now() / 1000);
    svc.setHostGuests([
      guest({ public_key: "old", expires_at: now - 5 }),
      guest({ public_key: "in", entered: true, expires_at: null }),
    ]);
    const { result } = renderHook(() => useHostGuests(svc));
    await waitFor(() => expect(result.current.guests.map((g) => g.public_key)).toEqual(["in"]));
    svc.setHostGuests([guest({ public_key: "new", expires_at: now + 100 })]);
    await waitFor(() => expect(result.current.guests.map((g) => g.public_key)).toEqual(["new"]));
  });
});
