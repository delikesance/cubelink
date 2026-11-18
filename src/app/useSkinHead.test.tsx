import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createFakeServices } from "../services/fake";
import { SKIN_RETRY_MS, useSkinHead } from "./useSkinHead";

const HEAD = "data:image/png;base64,AAAA";

describe("useSkinHead", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("réessaie quand la tête n'est pas là (réseau pas prêt), puis l'affiche", async () => {
    const services = createFakeServices({ tickMs: 0, requestCount: 0 });
    const answers: Array<string | null> = [null, null, HEAD];
    let n = 0;
    const calls = vi.fn(async () => answers[Math.min(n++, answers.length - 1)]);
    services.skinHeadByName = calls;
    const { result } = renderHook(() => useSkinHead(services, "name", "garder500"));
    await act(async () => { await Promise.resolve(); });
    expect(result.current).toBeNull();
    expect(calls).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(SKIN_RETRY_MS[0]); });
    expect(calls).toHaveBeenCalledTimes(2);
    expect(result.current).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(SKIN_RETRY_MS[1]); });
    expect(calls).toHaveBeenCalledTimes(3);
    expect(result.current).toBe(HEAD);
    // trouvée : plus aucune tentative
    await act(async () => { await vi.advanceTimersByTimeAsync(SKIN_RETRY_MS[2] * 2); });
    expect(calls).toHaveBeenCalledTimes(3);
  });

  it("s'arrête après le dernier délai, et plus rien après le démontage", async () => {
    const services = createFakeServices({ tickMs: 0, requestCount: 0 });
    const calls = vi.fn(async () => null);
    services.skinHeadByName = calls;
    const { unmount } = renderHook(() => useSkinHead(services, "name", "inconnu"));
    await act(async () => { await vi.advanceTimersByTimeAsync(SKIN_RETRY_MS.reduce((a, b) => a + b, 0) + 1000); });
    expect(calls).toHaveBeenCalledTimes(1 + SKIN_RETRY_MS.length);
    const services2 = createFakeServices({ tickMs: 0, requestCount: 0 });
    const calls2 = vi.fn(async () => null);
    services2.skinHeadByName = calls2;
    const r2 = renderHook(() => useSkinHead(services2, "name", "autre"));
    await act(async () => { await Promise.resolve(); });
    r2.unmount();
    await act(async () => { await vi.advanceTimersByTimeAsync(SKIN_RETRY_MS[0] * 3); });
    expect(calls2).toHaveBeenCalledTimes(1);
    unmount();
  });
});
