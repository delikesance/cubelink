import type { TransferProgress } from "../../services/types";

const UNITS = ["o", "Ko", "Mo", "Go", "To"];
const nf = new Intl.NumberFormat("fr-FR", { maximumFractionDigits: 1 });
const nf0 = new Intl.NumberFormat("fr-FR", { maximumFractionDigits: 0 });

/** Taille lisible, unités décimales françaises : « 340 Mo », « 1,2 Go ». */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 o";
  let i = 0;
  let v = n;
  while (v >= 1000 && i < UNITS.length - 1) { v /= 1000; i++; }
  return `${(i === 0 || v >= 100 ? nf0 : nf).format(v)} ${UNITS[i]}`;
}

export function formatSpeed(bps: number): string {
  return `${formatBytes(bps)}/s`;
}

/** Durée restante estimée, arrondie : « environ 1 min restante ». */
export function formatEta(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return "";
  if (seconds < 10) return "quelques secondes restantes";
  if (seconds < 60) return `environ ${Math.round(seconds / 5) * 5} s restantes`;
  const min = Math.round(seconds / 60);
  if (min < 60) return `environ ${min} min restante${min > 1 ? "s" : ""}`;
  const h = Math.floor(min / 60);
  const m = min % 60;
  return m ? `environ ${h} h ${m} min restantes` : `environ ${h} h restantes`;
}

export function percent(p: Pick<TransferProgress, "bytes_done" | "bytes_total" | "files_done" | "files_total">): number {
  const r = p.bytes_total > 0 ? p.bytes_done / p.bytes_total : p.files_total > 0 ? p.files_done / p.files_total : 0;
  return Math.max(0, Math.min(100, Math.floor(r * 100)));
}

/** « 12 / 40 fichiers · 340 Mo / 1,2 Go · 8 Mo/s » (la vitesse n'apparaît que si elle est connue). */
export function progressLabel(p: TransferProgress): string {
  const parts = [
    `${p.files_done} / ${p.files_total} fichiers`,
    `${formatBytes(p.bytes_done)} / ${formatBytes(p.bytes_total)}`,
  ];
  if (p.speed_bps > 0) parts.push(formatSpeed(p.speed_bps));
  return parts.join(" · ");
}

/** Moyenne mobile exponentielle de la vitesse, pour une estimation stable. */
export function smoothSpeed(prev: number | null, sample: number, alpha = 0.2): number | null {
  if (!(sample > 0)) return prev;
  return prev === null ? sample : prev + alpha * (sample - prev);
}

export function etaSeconds(p: TransferProgress, smoothed: number | null): number | null {
  if (!smoothed || smoothed <= 0 || p.bytes_total <= 0) return null;
  return Math.max(0, (p.bytes_total - p.bytes_done) / smoothed);
}

export type FailureKind = "friend-left" | "bad-hash" | "disk-full" | "java-missing" | "offline";

/** Identifie la cause d'un échec d'après son message ; null si non reconnu. */
export function classifyFailure(message: string | undefined): FailureKind | null {
  if (!message) return null;
  const m = message.toLowerCase();
  if (/disk|disque|espace|enospc|no space/.test(m)) return "disk-full";
  if (/hash|integrit|intégrit|corrompu|checksum|mismatch/.test(m)) return "bad-hash";
  if (/java/.test(m)) return "java-missing";
  if (/offline|hors ligne|r[eé]seau|network/.test(m)) return "offline";
  if (/friend|ami|peer|left|d[eé]connect|parti/.test(m)) return "friend-left";
  return null;
}
