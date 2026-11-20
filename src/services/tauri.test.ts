import { describe, expect, it } from "vitest";
import { toLeaveError } from "./tauri";
import { HearthLeaveError } from "./types";

describe("toLeaveError", () => {
  it("reconnaît les quatre codes de hearth_leave", () => {
    const a = toLeaveError({ code: "last_member" });
    const b = toLeaveError({ code: "unreachable" });
    const c = toLeaveError({ code: "other", message: "Quota atteint." });
    const d = toLeaveError({ code: "outbox_pending" });
    expect(a).toBeInstanceOf(HearthLeaveError);
    expect((a as HearthLeaveError).code).toBe("last_member");
    expect((b as HearthLeaveError).code).toBe("unreachable");
    expect((c as HearthLeaveError).code).toBe("other");
    expect(d).toBeInstanceOf(HearthLeaveError);
    expect((d as HearthLeaveError).code).toBe("outbox_pending");
    expect(c.message).toBe("Quota atteint.");
  });

  it("une erreur sans code (chaîne, Error) reste « other », jamais « unreachable »", () => {
    for (const e of ["Disque plein.", new Error("Disque plein."), { code: "inconnu" }]) {
      const r = toLeaveError(e) as HearthLeaveError;
      expect(r.code).toBe("other");
    }
    expect(toLeaveError("Disque plein.").message).toBe("Disque plein.");
  });
});
