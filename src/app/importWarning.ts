// Les mods manquants sont signalés pendant l'import (écran Create), qui navigue ensuite vers Main :
// on garde l'avertissement ici pour que Main puisse l'afficher.
let pending: string[] | null = null;

export function setImportWarning(missing: string[] | null): void {
  pending = missing && missing.length ? missing : null;
}

export function peekImportWarning(): string[] | null {
  return pending;
}
