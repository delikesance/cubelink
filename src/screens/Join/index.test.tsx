import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices, FAKE_MANIFEST_HASH } from "../../services/fake";
import type { Services } from "../../services";
import type { TransferProgress } from "../../services/types";
import Join from "./index";

afterEach(cleanup);

function setup(over: Partial<Services> = {}, params: Record<string, string> = { friend: "pk-alex" }) {
  const services = { ...createFakeServices({ tickMs: 0 }), ...over };
  let cb: (p: TransferProgress) => void = () => {};
  services.onTransferProgress = (c) => { cb = c; return () => {}; };
  const navigate = vi.fn();
  const goBack = vi.fn();
  const value = { services, route: "join", params, navigate, goBack, canGoBack: false, setTitle: () => {} } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Join /></ShellContext.Provider>);
  const emit = (p: TransferProgress) => act(() => cb(p));
  return { services, navigate, goBack, emit };
}

const P = (o: Partial<TransferProgress> & { error?: string } = {}): TransferProgress => ({
  phase: "receiving", files_done: 12, files_total: 40, bytes_done: 340_000_000, bytes_total: 1_200_000_000,
  speed_bps: 8_000_000, via_relay: false, ...o,
});

async function startJoin() {
  await screen.findByText("mods/mod-maison.jar");
  fireEvent.click(screen.getByRole("checkbox", { name: /confiance/ }));
  fireEvent.click(screen.getByRole("button", { name: "Rejoindre et jouer" }));
  await screen.findByRole("progressbar");
}

describe("Join", () => {
  it("RAM : valeur des paramètres par défaut, ou valeur choisie transmise à joinStart", async () => {
    const start = vi.fn(async () => {});
    setup({ joinStart: start });
    await screen.findByText("mods/mod-maison.jar");
    const inherit = screen.getByRole("checkbox", { name: /Valeur des paramètres/ }) as HTMLInputElement;
    expect(inherit.checked).toBe(true);
    fireEvent.click(inherit);
    fireEvent.change(screen.getByRole("slider", { name: "Mémoire allouée (Go)" }), { target: { value: "10" } });
    await startJoin();
    expect(start).toHaveBeenCalledWith("pk-alex", FAKE_MANIFEST_HASH, true, 10240);
  });

  it("affiche le plan sans rien télécharger, titre focalisé", async () => {
    const start = vi.fn(async () => {});
    setup({ joinStart: start });
    await screen.findByRole("button", { name: "Rejoindre et jouer" });
    await waitFor(() => expect(document.activeElement?.tagName).toBe("H1"));
    expect(screen.getByRole("heading", { level: 1 }).textContent).toMatch(/^Rejoindre /);
    expect(screen.getByText(/Minecraft \d/)).toBeTruthy();
    expect(screen.getByText(/Java \d+/)).toBeTruthy();
    expect(document.querySelector(".join-body")?.textContent).toContain("à recevoir");
    expect(screen.getByText(/Liste des fichiers \(/)).toBeTruthy();
    expect(start).not.toHaveBeenCalled();
  });

  it("signale les mods inconnus et exige la confiance", async () => {
    setup();
    expect(await screen.findByText("mods/mod-maison.jar")).toBeTruthy();
    expect(screen.getByText(/L'intégrité est vérifiée, pas la sécurité/)).toBeTruthy();
    const btn = screen.getByRole("button", { name: "Rejoindre et jouer" }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: /confiance/ }));
    expect(btn.disabled).toBe(false);
  });

  it("Rejoindre appelle joinStart puis affiche la progression dans la même page", async () => {
    const { services, navigate, emit } = setup({ joinPlay: vi.fn(async () => {}) });
    const spy = vi.spyOn(services, "joinStart");
    await startJoin();
    expect(spy).toHaveBeenCalledWith("pk-alex", FAKE_MANIFEST_HASH, true, 0);
    expect(navigate).not.toHaveBeenCalled();
    emit(P());
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("28");
    expect(screen.getByText("2. Réception des fichiers").closest("li")?.getAttribute("aria-current")).toBe("step");
    expect(screen.queryByRole("button", { name: "Rejoindre et jouer" })).toBeNull();
  });

  it("fin du transfert : joinPlay automatique puis accueil", async () => {
    const play = vi.fn(async (_f: string, _i: string) => {});
    const { navigate, emit } = setup({ joinPlay: play });
    await startJoin();
    emit(P({ phase: "done", files_done: 40, bytes_done: 1_200_000_000, speed_bps: 0 }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
    expect(play).toHaveBeenCalledTimes(1);
    expect(play.mock.calls[0][0]).toBe("pk-alex");
    // Instance invitée : désignée par le backend (vide), jamais la première instance de la liste.
    expect(play.mock.calls[0][1]).toBe("");
  });

  it("échec de la connexion au jeu : message sur place puis Réessayer", async () => {
    const play = vi.fn().mockRejectedValueOnce(new Error("Ton ami est hors ligne.")).mockResolvedValue(undefined);
    const { navigate, emit } = setup({ joinPlay: play });
    await startJoin();
    emit(P({ phase: "done" }));
    expect((await screen.findAllByRole("alert")).some((a) => /est hors ligne\./.test(a.textContent ?? ""))).toBe(true);
    expect(navigate).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
  });

  it("échec identifié : erreur sur place (textes d'Erreur), pas de navigation, Réessayer relance joinStart", async () => {
    const start = vi.fn(async () => {});
    const { navigate, emit } = setup({ joinStart: start });
    await startJoin();
    emit(P({ phase: "failed", error: "intégrité : aero.jar corrompu" }));
    expect(screen.getByRole("alert").textContent).toContain("aero.jar");
    expect(screen.getByRole("alert").textContent).toMatch(/ne correspond pas/);
    expect(navigate).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("status").textContent).toBe("Préparation en cours");
  });

  it("Réessayer après un changement chez l'ami : nouveau plan, et retour au plan s'il y a des fichiers à accepter", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const start = vi.fn(async () => {});
    const plan = vi.fn(() => base.joinPlan("x"));
    const { emit } = setup({ joinStart: start, joinPlan: plan });
    await startJoin();
    plan.mockImplementation(async () => ({ ...(await base.joinPlan("x")), manifest_hash: "e".repeat(64) }));
    emit(P({ phase: "failed", error: "Un fichier a changé chez ton ami pendant le transfert : réessaie dans un instant" }));
    fireEvent.click(await screen.findByRole("button", { name: "Réessayer" }));
    expect(await screen.findByText(/a changé : vérifie le nouveau plan/)).toBeTruthy();
    expect(start).toHaveBeenCalledTimes(1);
    expect((screen.getByRole("checkbox", { name: /confiance/ }) as HTMLInputElement).checked).toBe(false);
  });

  it("échec inconnu : message réel, Retour à l'accueil", async () => {
    const { navigate, emit } = setup();
    await startJoin();
    emit(P({ phase: "failed", error: "Réception : réponse inattendue" }));
    expect(screen.getByRole("alert").textContent).toContain("Réception : réponse inattendue");
    fireEvent.click(screen.getByRole("button", { name: "Retour à l'accueil" }));
    expect(navigate).toHaveBeenCalledWith("main");
  });

  it("annulation pendant le transfert : confirmation puis joinCancel, Échap sans effet", async () => {
    const cancel = vi.fn(async () => {});
    const { navigate, goBack, emit } = setup({ joinCancel: cancel });
    await startJoin();
    emit(P());
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(goBack).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    fireEvent.click(screen.getByRole("button", { name: "Continuer l'installation" }));
    expect(cancel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    fireEvent.click(screen.getByRole("button", { name: "Oui, annuler" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
    expect(cancel).toHaveBeenCalled();
  });

  it("échec de joinStart : erreur sur le plan, pas de transfert", async () => {
    setup({ joinStart: vi.fn().mockRejectedValue(new Error("Ton ami est hors ligne.")) });
    await screen.findByText("mods/mod-maison.jar");
    fireEvent.click(screen.getByRole("checkbox", { name: /confiance/ }));
    fireEvent.click(screen.getByRole("button", { name: "Rejoindre et jouer" }));
    expect(await screen.findByText(/est hors ligne\./)).toBeTruthy();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("sans mod inconnu, Rejoindre est actif directement", async () => {
    const base = createFakeServices({ tickMs: 0 });
    setup({ joinPlan: async () => ({ ...(await base.joinPlan("x")), unknown_mods: [] }) });
    await screen.findByRole("button", { name: "Rejoindre et jouer" });
    expect((screen.getByRole("button", { name: "Rejoindre et jouer" }) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByRole("checkbox", { name: /confiance/ })).toBeNull();
  });

  it("mod inconnu déjà accepté : pas de case de confiance, simple mention", async () => {
    const base = createFakeServices({ tickMs: 0 });
    setup({ joinPlan: async () => ({ ...(await base.joinPlan("x")), unknown_mods: [], unknown_trusted: ["mods/mod-maison.jar"] }) });
    await screen.findByRole("button", { name: "Rejoindre et jouer" });
    expect((screen.getByRole("button", { name: "Rejoindre et jouer" }) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByRole("checkbox", { name: /confiance/ })).toBeNull();
    expect(screen.getByTestId("already-trusted").textContent).toMatch(/déjà été accepté/);
  });

  it("Annuler et Échap reviennent en arrière (fallback main)", async () => {
    const { goBack } = setup();
    await screen.findByRole("button", { name: "Rejoindre et jouer" });
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(goBack).toHaveBeenCalledWith("main");
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(goBack).toHaveBeenCalledTimes(2);
  });

  it("clé absente : message et retour", () => {
    const { goBack } = setup({}, {});
    expect(screen.getByRole("alert").textContent).toContain("Ami introuvable");
    fireEvent.click(screen.getByRole("button", { name: "Retour à l'accueil" }));
    expect(goBack).toHaveBeenCalledWith("main");
  });

  it("erreur puis Réessayer", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const plan = vi.fn()
      .mockRejectedValueOnce(new Error("Ton ami est hors ligne."))
      .mockImplementation(() => base.joinPlan("x"));
    setup({ joinPlan: plan });
    expect(await screen.findByText("Ton ami est hors ligne.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(await screen.findByRole("button", { name: "Rejoindre et jouer" })).toBeTruthy();
    expect(plan).toHaveBeenCalledTimes(2);
  });

  /** S1 (T2) : un jar non vérifié (p. ex. sans sha1 dans le manifeste) exige la case de confiance, comme un inconnu. */
  it("jars non vérifiés : traités comme inconnus, la case de confiance est obligatoire", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const start = vi.fn(async () => {});
    setup({ joinStart: start, joinPlan: async () => ({ ...(await base.joinPlan("x")), unknown_mods: [], unknown_unchecked: ["mods/x.jar"] }) });
    expect(await screen.findByText("mods/x.jar")).toBeTruthy();
    expect(screen.getByTestId("unchecked").textContent).toMatch(/traités comme inconnus/);
    const btn = screen.getByRole("button", { name: "Rejoindre et jouer" }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: /confiance/ }));
    fireEvent.click(btn);
    await waitFor(() => expect(start).toHaveBeenCalledWith("pk-alex", FAKE_MANIFEST_HASH, true, 0));
  });

  it("scripts KubeJS : la case de confiance est obligatoire", async () => {
    const base = createFakeServices({ tickMs: 0 });
    setup({ joinPlan: async () => ({ ...(await base.joinPlan("x")), unknown_mods: [], unknown_scripts: ["kubejs/server_scripts/a.js"] }) });
    expect(await screen.findByText("kubejs/server_scripts/a.js")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Rejoindre et jouer" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("le monde a changé depuis le plan : message et nouveau plan à revérifier", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const plan = vi.fn(() => base.joinPlan("x"));
    setup({ joinPlan: plan, joinStart: vi.fn().mockRejectedValue(new Error("La partie de ton ami a changé depuis l'affichage du plan : vérifie le nouveau plan avant de rejoindre.")) });
    await screen.findByText("mods/mod-maison.jar");
    fireEvent.click(screen.getByRole("checkbox", { name: /confiance/ }));
    fireEvent.click(screen.getByRole("button", { name: "Rejoindre et jouer" }));
    expect(await screen.findByText(/a changé depuis l'affichage du plan/)).toBeTruthy();
    await waitFor(() => expect(plan).toHaveBeenCalledTimes(2));
    await screen.findByText("mods/mod-maison.jar");
    expect((screen.getByRole("checkbox", { name: /confiance/ }) as HTMLInputElement).checked).toBe(false);
  });
});
