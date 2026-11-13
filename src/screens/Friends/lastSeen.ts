/** « Vu il y a 3 h » : dernière connexion d'un ami hors ligne, relative à `nowSec` (secondes Unix). */
export function lastSeenText(seen: number | null | undefined, nowSec: number): string | null {
  if (!seen) return null;
  const d = Math.max(0, nowSec - seen);
  if (d < 60) return "Vu à l'instant";
  if (d < 3600) return `Vu il y a ${Math.floor(d / 60)} min`;
  if (d < 86400) return `Vu il y a ${Math.floor(d / 3600)} h`;
  const days = Math.floor(d / 86400);
  if (days === 1) return "Vu hier";
  if (days < 30) return `Vu il y a ${days} jours`;
  const date = new Date(seen * 1000);
  const sameYear = date.getFullYear() === new Date(nowSec * 1000).getFullYear();
  return `Vu le ${date.toLocaleDateString("fr-FR", sameYear ? { day: "numeric", month: "long" } : { day: "numeric", month: "long", year: "numeric" })}`;
}
