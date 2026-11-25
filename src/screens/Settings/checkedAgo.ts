/** « Vérifié il y a 2 h » : âge du dernier contrôle de mise à jour, relatif à `nowSec` (secondes Unix). Jamais négatif (horloge en retard). */
export function checkedAgo(checked: number, nowSec: number): string {
  const d = Math.max(0, nowSec - checked);
  if (d < 60) return "Vérifié à l'instant";
  if (d < 3600) return `Vérifié il y a ${Math.floor(d / 60)} min`;
  if (d < 86400) return `Vérifié il y a ${Math.floor(d / 3600)} h`;
  const days = Math.floor(d / 86400);
  return days === 1 ? "Vérifié hier" : `Vérifié il y a ${days} jours`;
}
