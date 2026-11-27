import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices, FAKE_MANIFEST_HASH } from "../../services/fake";
import type { Services } from "../../services";
import type { TransferProgress } from "../../services/types";
import { goBack as routerGoBack, resetHistory } from "../../app/router";
import Join, { ramHint } from "./index";

afterEach(() => { cleanup(); resetHistory(); });

describe("ramHint", () => {
  const pack = (n: number) => Array.from({ length: n }, (_, i) => ({ path: `mods/m${i}.jar` })) as unknown as Parameters<typeof ramHint>[0];
  it("suit les seuils de la mémoire Automatique (100 et 250 mods)", () => {
    expect(ramHint(pack(99))).toBeUndefined();
    expect(ramHint(pack(100))).toMatch(/6 Go/);
    expect(ramHint(pack(249))).toMatch(/6 Go/);
    expect(ramHint(pack(250))).toMatch(/8 Go/);
  });
});

function setup(over: Partial<Services> = {}, params: Record<string, string> = { friend: "pk-alex" }) {
  const services = { ...createFakeServices({ tickMs: 0 }), ...over };
  let cb: (p: TransferProgress) => void = () => {};
  services.onTransferProgress = (c) => { cb = c; return () => {}; };
  const navigate = vi.fn();
  const goBack = vi.fn();
  const value = { services, route: "join", params, navigate, goBack, canGoBack: false, setTitle: () => {} } as ShellContextValue;
  const ui = (v: ShellContextValue) => <ShellContext.Provider value={v}><Join /></ShellContext.Provider>;
  const { unmount, rerender } = render(ui(value));
  const emit = (p: TransferProgress) => act(() => cb(p));
  const setParams = (next: Record<string, string>) => rerender(ui({ ...value, params: next }));
  return { services, navigate, goBack, emit, unmount, setParams };
}

const P = (o: Partial<TransferProgress> & { error?: string } = {}): TransferProgress => ({
  phase: "receiving", files_done: 12, files_total: 40, bytes_done: 340_000_000, bytes_total: 1_200_000_000,
  speed_bps: 8_000_000, ...o,
});

async function startJoin() {
  await screen.findByText("mods/mod-maison.jar");
  fireEvent.click(screen.getByRole("checkbox", { name: /confiance/ }));
  fireEvent.click(screen.getByRole("button", { name: "Rejoindre et jouer" }));
  await screen.findByRole("progressbar");
}

describe("Join", () => {
  it("avertit du disque et du compte hors ligne sans bloquer « Rejoindre et jouer »", async () => {
    const accounts = [{ id: "off-notch", kind: "offline" as const, name: "Notch", uuid: "u" }];
    const { navigate } = setup(createFakeServices({ tickMs: 0, accounts, freeDiskBytes: 500_000_000 }));
    await screen.findByText("mods/mod-maison.jar");
    const box = screen.getByTestId("join-warnings");
    expect(box.getAttribute("role")).toBe("status");
    expect(screen.getByTestId("warn-disk").textContent).toMatch(/Il te faut 1,1 Go libres, il en reste 500 Mo/);
    expect(screen.getByTestId("warn-offline").textContent).toMatch(/compte hors ligne/);
    fireEvent.click(screen.getByRole("checkbox", { name: /confiance/ }));
    expect((screen.getByRole("button", { name: "Rejoindre et jouer" }) as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Changer de compte" }));
    expect(navigate).toHaveBeenCalledWith("settings", { tab: "compte" });
  });

  it("arrondit le besoin au-dessus et l'espace libre au-dessous (jamais « 2 Go libres, il en reste 2 Go »)", async () => {
    // Libre juste sous le besoin (reçu + 1 Gio) : les deux tailles doivent différer à l'affichage.
    setup(createFakeServices({ tickMs: 0, freeDiskBytes: 1_073_741_900 }));
    await screen.findByText("mods/mod-maison.jar");
    const txt = screen.getByTestId("warn-disk").textContent ?? "";
    const [, need, free] = txt.match(/Il te faut ([\d,]+) Go libres, il en reste ([\d,]+) Go/) ?? [];
    expect(Number(need.replace(",", "."))).toBeGreaterThan(Number(free.replace(",", ".")));
  });

  it("aucun bloc d'avertissement sans disque trop juste ni compte hors ligne", async () => {
    setup(createFakeServices({ tickMs: 0, freeDiskBytes: 50_000_000_000 }));
    await screen.findByText("mods/mod-maison.jar");
    expect(screen.queryByTestId("join-warnings")).toBeNull();
  });

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

  it("mods inconnus : avertissement et case de confiance dans la carte, avant les versions et le bouton", async () => {
    setup();
    await screen.findByText("mods/mod-maison.jar");
    const card = document.querySelector(".join-card") as HTMLElement;
    const warn = card.querySelector(".join-warn") as HTMLElement;
    const body = card.querySelector(".join-body") as HTMLElement;
    const trust = screen.getByRole("checkbox", { name: /confiance/ });
    const btn = screen.getByRole("button", { name: "Rejoindre et jouer" });
    const before = (a: Node, b: Node) => !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    expect(card.contains(warn)).toBe(true);
    expect(card.contains(trust)).toBe(true);
    // Avertissement après le titre, avant les versions et le résumé ; case avant le bouton.
    expect(before(card.querySelector(".join-who")!, warn)).toBe(true);
    expect(before(warn, body)).toBe(true);
    // La case est dans l'avertissement qu'elle accepte, donc visible avec la liste, et avant le bouton qu'elle débloque.
    expect(warn.contains(trust)).toBe(true);
    expect(before(trust, btn)).toBe(true);
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

  it("écran quitté pendant le lancement : joinPlay se termine sans renvoyer à l'accueil", async () => {
    let finish: () => void = () => {};
    const play = vi.fn(() => new Promise<void>((r) => { finish = r; }));
    const { navigate, emit, unmount } = setup({ joinPlay: play });
    await startJoin();
    emit(P({ phase: "done" }));
    await waitFor(() => expect(play).toHaveBeenCalledTimes(1));
    unmount();
    await act(async () => { finish(); await Promise.resolve(); });
    expect(navigate).not.toHaveBeenCalled();
  });

  it("écran quitté pendant le lancement : un échec de joinPlay est avalé sans erreur ni navigation", async () => {
    let fail: (e: Error) => void = () => {};
    const play = vi.fn(() => new Promise<void>((_, r) => { fail = r; }));
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    const { navigate, emit, unmount } = setup({ joinPlay: play });
    await startJoin();
    emit(P({ phase: "done" }));
    await waitFor(() => expect(play).toHaveBeenCalledTimes(1));
    unmount();
    await act(async () => { fail(new Error("boum")); await Promise.resolve(); });
    expect(navigate).not.toHaveBeenCalled();
    expect(err).not.toHaveBeenCalled();
    err.mockRestore();
  });

  it("autre ami ouvert pendant le lancement (écran non démonté) : pas de retour forcé à l'accueil", async () => {
    let finish: () => void = () => {};
    const play = vi.fn(() => new Promise<void>((r) => { finish = r; }));
    const { navigate, emit, setParams } = setup({ joinPlay: play });
    await startJoin();
    emit(P({ phase: "done" }));
    await waitFor(() => expect(play).toHaveBeenCalledTimes(1));
    setParams({ friend: "pk-autre" });
    await act(async () => { finish(); await Promise.resolve(); });
    expect(navigate).not.toHaveBeenCalled();
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

  it("panneau de transfert : barre accessible, annonce live limitée à la phase, préparation et lancement", async () => {
    const { emit } = setup();
    await startJoin();
    expect(screen.getByRole("status").textContent).toBe("Préparation en cours");
    emit(P());
    const bar = screen.getByRole("progressbar");
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuemax")).toBe("100");
    expect((bar.getAttribute("aria-valuetext") ?? "").replace(/[\u202f\u00a0]/g, " ")).toContain("12 / 40 fichiers · 340 Mo / 1,2 Go · 8 Mo/s");
    const live = screen.getByRole("status");
    expect(live.textContent).toBe("Réception des fichiers");
    emit(P({ files_done: 20, bytes_done: 500_000_000 }));
    expect(live.textContent).toBe("Réception des fichiers");
    emit(P({ phase: "verifying" }));
    expect(live.textContent).toBe("Vérification d'intégrité");
    emit(P({ phase: "launching" }));
    expect(live.textContent).toBe("Lancement du jeu");
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
  });

  it("pause puis reprise : joinPause / joinResume, l'étape en cours reste la réception", async () => {
    const pause = vi.fn(async () => {});
    const resume = vi.fn(async () => {});
    const { emit } = setup({ joinPause: pause, joinResume: resume });
    await startJoin();
    emit(P());
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(pause).toHaveBeenCalled());
    emit(P({ phase: "paused", speed_bps: 0 }));
    expect(screen.getByRole("status").textContent).toBe("Transfert en pause");
    expect(screen.getByText("2. Réception des fichiers").closest("li")?.getAttribute("aria-current")).toBe("step");
    fireEvent.click(screen.getByRole("button", { name: "Reprendre" }));
    await waitFor(() => expect(resume).toHaveBeenCalled());
  });

  it("échec : message réel, bloc Détails et chemin du journal", async () => {
    let det: (e: { step: string; message: string; details: string; log_file: string }) => void = () => {};
    const { emit } = setup({ onTransferErrorDetails: (c: typeof det) => { det = c; return () => {}; } } as never);
    await startJoin();
    act(() => det({ step: "Réception", message: "Réception : réponse inattendue de l'hôte", details: "étape: Réception | détail: reset", log_file: "C:\\logs\\join-errors.log" }));
    emit(P({ phase: "failed", error: "Réception : réponse inattendue de l'hôte" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/^Réception : réponse inattendue de l'hôte/);
    expect(screen.getByText("Détails")).toBeTruthy();
    expect(screen.getByLabelText("Détails de l'erreur").textContent).toContain("détail: reset");
    expect(screen.getByRole("button", { name: "Copier" })).toBeTruthy();
    expect(screen.getByText(/join-errors\.log/)).toBeTruthy();
  });

  it("échec sur Java ou le réseau : message réel sur place, jamais pris pour un départ de l'ami", async () => {
    const { navigate, emit } = setup();
    await startJoin();
    emit(P({ phase: "failed", error: "Java 21 introuvable : l'ami attend" }));
    expect(screen.getByRole("alert").textContent).toContain("Java 21 introuvable");
    expect(screen.queryByText(/s'est déconnecté/)).toBeNull();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("mods inconnus reçus pendant le transfert (transfer://unknown_mods) : liste affichée", async () => {
    let push: (e: { instance_id: string; unknown_mods: string[] }) => void = () => {};
    setup({ onUnknownMods: (cb) => { push = cb; return () => {}; } });
    await startJoin();
    act(() => push({ instance_id: "i1", unknown_mods: ["mods/maison-recue.jar"] }));
    expect(screen.getByRole("note").textContent).toContain("mods/maison-recue.jar");
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

  describe("Retour pendant le transfert (vrai routeur)", () => {
    const back = () => act(() => { routerGoBack(); });

    it("Retour, Alt+← ou souris 3 ouvrent la confirmation ; Continuer garde le transfert et le lancement auto", async () => {
      const cancel = vi.fn(async () => {});
      const play = vi.fn(async (_f: string, _i: string) => {});
      const { navigate, emit } = setup({ joinCancel: cancel, joinPlay: play });
      await startJoin();
      emit(P());
      expect(screen.queryByText(/Annuler l'installation/)).toBeNull();
      back();
      expect(screen.getByText(/Annuler l'installation/)).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Continuer l'installation" }));
      expect(screen.queryByText(/Annuler l'installation/)).toBeNull();
      expect(cancel).not.toHaveBeenCalled();
      expect(navigate).not.toHaveBeenCalled();
      // même chaîne que le Shell : Alt+← et souris passent par goBack()
      const onKey = (e: KeyboardEvent) => { if (e.altKey && e.key === "ArrowLeft") routerGoBack(); };
      window.addEventListener("keydown", onKey);
      fireEvent.keyDown(window, { key: "ArrowLeft", altKey: true });
      window.removeEventListener("keydown", onKey);
      expect(screen.getByText(/Annuler l'installation/)).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Continuer l'installation" }));
      // le transfert se termine : confirmation fermée, lancement automatique conservé
      emit(P({ phase: "done", files_done: 40, bytes_done: 1_200_000_000, speed_bps: 0 }));
      await waitFor(() => expect(play).toHaveBeenCalledTimes(1));
      await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
    });

    it("« Oui, annuler » : joinCancel puis accueil", async () => {
      const cancel = vi.fn(async () => {});
      const { navigate, emit } = setup({ joinCancel: cancel });
      await startJoin();
      emit(P());
      back();
      fireEvent.click(screen.getByRole("button", { name: "Oui, annuler" }));
      await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
      expect(cancel).toHaveBeenCalledTimes(1);
    });

    it("pendant la préparation (avant le premier événement), Retour est déjà gardé", async () => {
      setup();
      await startJoin();
      back();
      expect(screen.getByText(/Annuler l'installation/)).toBeTruthy();
    });

    it("échec ou fin : la confirmation se ferme et Retour redevient libre ; Réessayer réarme la garde", async () => {
      const { emit } = setup();
      window.location.hash = "#/main"; resetHistory();
      window.location.hash = "#/join?friend=pk-alex"; window.dispatchEvent(new HashChangeEvent("hashchange"));
      await startJoin();
      emit(P());
      back();
      expect(screen.getByText(/Annuler l'installation/)).toBeTruthy();
      emit(P({ phase: "failed", error: "Réception : réponse inattendue" }));
      expect(screen.queryByText(/Annuler l'installation/)).toBeNull();
      back();
      expect(window.location.hash).toBe("#/main");
    });

    it("démonté : la garde est levée", async () => {
      const { emit, unmount } = setup();
      window.location.hash = "#/main"; resetHistory();
      window.location.hash = "#/join?friend=pk-alex"; window.dispatchEvent(new HashChangeEvent("hashchange"));
      await startJoin();
      emit(P());
      unmount();
      back();
      expect(window.location.hash).toBe("#/main");
    });
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

  it("ami de confiance : pas de case pour ses jars, note friend-trusted et acceptUnknown=false", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const noor = (await base.friendList()).find((f) => f.friend.trust === "always")!;
    const start = vi.fn(base.joinStart);
    setup({ ...base, joinStart: start }, { friend: noor.friend.public_key });
    const btn = (await screen.findByRole("button", { name: "Rejoindre et jouer" })) as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    expect(screen.queryByRole("checkbox", { name: /confiance/ })).toBeNull();
    expect(screen.getByTestId("friend-trusted").textContent).toContain(noor.friend.name);
    fireEvent.click(btn);
    await waitFor(() => expect(start).toHaveBeenCalledWith(noor.friend.public_key, FAKE_MANIFEST_HASH, false, 0));
  });

  it("ami de confiance avec script KubeJS : la case reste exigée pour le script et friend-trusted est affiché", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const noor = (await base.friendList()).find((f) => f.friend.trust === "always")!;
    setup({
      ...base,
      joinPlan: async (pk) => ({
        ...(await base.joinPlan(pk)),
        unknown_scripts: ["kubejs/server_scripts/x.js"],
      }),
    }, { friend: noor.friend.public_key });
    expect(await screen.findByText("kubejs/server_scripts/x.js")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Rejoindre et jouer" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("friend-trusted").textContent).toContain(noor.friend.name);
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
