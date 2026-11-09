import { createFakeServices } from "./fake";
import { createTauriServices } from "./tauri";
import type { Services } from "./types";

export * from "./types";

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Vrai service dans Tauri, faux service en mémoire sinon (navigateur, tests). */
export function createServices(): Services {
  return isTauri() ? createTauriServices() : createFakeServices();
}

export { createFakeServices, createTauriServices };
