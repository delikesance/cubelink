// Les mods manquants sont signalés pendant l'import (écran Create), qui navigue ensuite vers Main :
// on garde l'avertissement ici pour que Main puisse l'afficher.
let pending: string[] | null = null;

export function setImportWarning(missing: string[] | null): void {
  pending = missing && missing.length ? missing : null;
}

export function peekImportWarning(): string[] | null {
  return pending;
}

// Même principe pour l'échec de préparation du jeu (réseau, Java) : l'instance existe, le jeu viendra au premier lancement.
let deferred: string | null = null;

export function setPrepareDeferred(message: string | null): void {
  deferred = message;
}

export function peekPrepareDeferred(): string | null {
  return deferred;
}
