/** Pseudo d'un compte hors ligne : 3 à 16 caractères parmi lettres, chiffres et `_`. Renvoie un message en français ou null. */
export function validateOfflineName(name: string): string | null {
  const n = name.trim();
  if (!n) return "Saisis un pseudo pour jouer hors ligne.";
  if (!/^[A-Za-z0-9_]{3,16}$/.test(n)) return "Le pseudo doit faire 3 à 16 caractères : lettres, chiffres ou _.";
  return null;
}
