import { describe, expect, it } from "vitest";
import { classifyFailure, etaSeconds, formatBytes, formatEta, formatSpeed, percent, progressLabel, smoothSpeed } from "./format";

const base = { phase: "receiving", files_done: 12, files_total: 40, bytes_done: 340_000_000, bytes_total: 1_200_000_000, speed_bps: 8_000_000, via_relay: false } as const;
const sp = (s: string) => s.replace(/[  ]/g, " ");

describe("format", () => {
  it("tailles en français", () => {
    expect(formatBytes(0)).toBe("0 o");
    expect(formatBytes(512)).toBe("512 o");
    expect(formatBytes(340_000_000)).toBe("340 Mo");
    expect(formatBytes(1_200_000_000)).toBe("1,2 Go");
    expect(formatSpeed(8_000_000)).toBe("8 Mo/s");
  });
  it("durées", () => {
    expect(formatEta(null)).toBe("");
    expect(formatEta(4)).toBe("quelques secondes restantes");
    expect(formatEta(33)).toBe("environ 35 s restantes");
    expect(formatEta(60)).toBe("environ 1 min restante");
    expect(formatEta(300)).toBe("environ 5 min restantes");
    expect(formatEta(7500)).toBe("environ 2 h 5 min restantes");
  });
  it("libellé et pourcentage", () => {
    expect(sp(progressLabel(base))).toBe("12 / 40 fichiers · 340 Mo / 1,2 Go · 8 Mo/s");
    expect(sp(progressLabel({ ...base, speed_bps: 0 }))).toBe("12 / 40 fichiers · 340 Mo / 1,2 Go");
    expect(percent(base)).toBe(28);
    expect(percent({ ...base, bytes_total: 0, files_done: 1, files_total: 4 })).toBe(25);
    expect(percent({ ...base, bytes_total: 0, files_total: 0 })).toBe(0);
  });
  it("lissage et estimation", () => {
    expect(smoothSpeed(null, 10)).toBe(10);
    expect(smoothSpeed(10, 20)).toBe(12);
    expect(smoothSpeed(10, 0)).toBe(10);
    expect(etaSeconds(base, 8_000_000)).toBeCloseTo(107.5);
    expect(etaSeconds(base, null)).toBeNull();
  });
  it("classe les échecs", () => {
    expect(classifyFailure("no space left on device")).toBe("disk-full");
    expect(classifyFailure("hash mismatch")).toBe("bad-hash");
    expect(classifyFailure("l'ami s'est déconnecté")).toBe("friend-left");
    expect(classifyFailure("boom")).toBeNull();
    expect(classifyFailure(undefined)).toBeNull();
  });
});
