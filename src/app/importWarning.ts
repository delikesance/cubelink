import type { ImportWarningEvent } from "../services/types";

// Les avertissements d'import (mods introuvables, fichiers ignorés) arrivent pendant l'import (écran Create), qui
// navigue ensuite vers Main : on garde l'avertissement ici pour que Main puisse l'afficher.
let pending: ImportWarningEvent | null = null;

export function setImportWarning(warning: ImportWarningEvent | null): void {
  pending = warning && (warning.missing.length || warning.skipped.length) ? warning : null;
}

export function peekImportWarning(): ImportWarningEvent | null {
  return pending;
}

function list(items: string[], max = 5): string {
  return items.slice(0, max).join(", ") + (items.length > max ? `… (+${items.length - max})` : "");
}

/** Phrase de fin d'import : mods introuvables, puis fichiers du pack ignorés par sécurité (avec leur raison). */
export function importWarningText(w: ImportWarningEvent): string {
  const parts: string[] = [];
  const m = w.missing.length;
  if (m) parts.push(`${m} mod${m > 1 ? "s" : ""} introuvable${m > 1 ? "s" : ""} : ${list(w.missing)}.`);
  const s = w.skipped.length;
  if (s) parts.push(`${s} fichier${s > 1 ? "s" : ""} du pack ${s > 1 ? "ont" : "a"} été ignoré${s > 1 ? "s" : ""} par sécurité : ${list(w.skipped)}.`);
  return `Import terminé, mais ${parts.join(" ")}`;
}
