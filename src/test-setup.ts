import { beforeAll } from "vitest";
import { preloadScreens } from "./app/shell";

// Les écrans sont chargés à la demande en production ; les tests les veulent disponibles de façon synchrone.
beforeAll(async () => { await preloadScreens(); });
