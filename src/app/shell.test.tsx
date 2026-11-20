import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Shell, useScreenTitle } from "./shell";
import { buildHash, navigate, parseHash, goBack, canGoBack, resetHistory } from "./router";
import { ShellContext, type ShellContextValue } from "./context";
import { createFakeServices } from "../services/fake";

function go(hash: string) {
  act(() => {
    window.location.hash = hash;
    window.dispatchEvent(new HashChangeEvent("hashchange"));
  });
}

beforeEach(() => { window.location.hash = ""; resetHistory(); });
afterEach(cleanup);

describe("routeur", () => {
  it("route par défaut : login", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(screen.getByRole("main").getAttribute("data-route")).toBe("login");
    expect(document.querySelector(".titlebar-title")!.textContent).toBe("Connexion");
  });

  it("navigation par hash et focus sur le contenu", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    go("#/legal");
    const main = screen.getByRole("main");
    expect(main.getAttribute("data-route")).toBe("legal");
    // le focus est dans le contenu : sur le h1 de l'écran s'il en a un (il le prend lui-même), sinon sur <main> (shell)
    const h1 = main.querySelector("h1");
    expect(document.activeElement).toBe(h1 ?? main);
    expect(document.querySelector(".titlebar-title")!.textContent).toBe("Mentions légales");
  });

  it("l'ancien écran Amis ouvre Paramètres > Amis", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    go("#/friends");
    expect(screen.getByRole("main").getAttribute("data-route")).toBe("settings");
    expect(screen.getByRole("tab", { name: "Amis" }).getAttribute("aria-selected")).toBe("true");
  });

  it("route inconnue : repli sur login", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    go("#/nimporte-quoi");
    expect(screen.getByRole("main").getAttribute("data-route")).toBe("login");
  });

  it("parse et construit les paramètres", () => {
    expect(parseHash("#/install?friend=abc")).toEqual({ route: "install", params: { friend: "abc" } });
    expect(buildHash("join", { friend: "x" })).toBe("#/join?friend=x");
  });

  it("lien d'évitement : donne le focus au contenu", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    fireEvent.click(screen.getByText("Aller au contenu"));
    expect(document.activeElement).toBe(screen.getByRole("main"));
  });
});

describe("titre contextuel", () => {
  it("modifiable par un écran, remis à zéro au démontage", () => {
    const calls: (string | null)[] = [];
    function Probe() { useScreenTitle("Monde de Alex"); return null; }
    const value: ShellContextValue = {
      services: createFakeServices({ tickMs: 0 }), route: "main", params: {},
      navigate, goBack, canGoBack: false, setTitle: (t) => { calls.push(t); },
    };
    const { unmount } = render(<ShellContext.Provider value={value}><Probe /></ShellContext.Provider>);
    expect(calls).toEqual(["Monde de Alex"]);
    unmount();
    expect(calls).toEqual(["Monde de Alex", null]);
  });
});

describe("barre de titre", () => {
  it("boutons Réduire / Fermer présents et sans effet hors Tauri", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    const min = screen.getByRole("button", { name: "Réduire" });
    const close = screen.getByRole("button", { name: "Fermer" });
    expect(() => { fireEvent.click(min); fireEvent.click(close); }).not.toThrow();
  });
});

describe("moments Hearth (écoutés sur toutes les routes)", () => {
  const hostPrompt = { group_id: "g1", name: "Les copains", players: 2 };
  const handover = { group_id: "g1", name: "Les copains", from_name: "Alex" };

  async function renderOn(hash: string, state: "hosting" | "idle" = "hosting") {
    window.location.hash = hash;
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "g1", name: "Les copains", state });
    services.setHearthEula(true);
    render(<Shell services={services} />);
    await act(async () => {});
    return services;
  }

  it.each(["#/settings", "#/instance", "#/legal"])("serveur resté ouvert sur %s : la boîte s'affiche", async (hash) => {
    const services = await renderOn(hash);
    act(() => { services.emitHearthHostPrompt(hostPrompt); });
    await screen.findByRole("alertdialog", { name: "Ton serveur est encore ouvert" });
  });

  it.each(["#/browse", "#/settings", "#/join"])("passage de relais sur %s : la proposition s'affiche", async (hash) => {
    const services = await renderOn(hash, "idle");
    act(() => { services.emitHearthHandover(handover); });
    const dlg = await screen.findByRole("dialog", { name: "L'hôte a arrêté le serveur" });
    expect(within(dlg).getByText(/Alex/)).toBeTruthy();
  });

  it("sur l'écran de connexion aussi (écoute dès le démarrage)", async () => {
    const services = await renderOn("");
    expect(screen.getByRole("main").getAttribute("data-route")).toBe("login");
    act(() => { services.emitHearthHostPrompt(hostPrompt); });
    await screen.findByRole("alertdialog", { name: "Ton serveur est encore ouvert" });
  });

  it.each(["#/main", "#/hearth?id=g1"])("un seul exemplaire de chaque boîte sur %s", async (hash) => {
    const services = await renderOn(hash);
    act(() => { services.emitHearthHostPrompt(hostPrompt); });
    expect(await screen.findAllByRole("alertdialog", { name: "Ton serveur est encore ouvert" })).toHaveLength(1);
    act(() => { services.emitHearthHandover(handover); });
    expect(await screen.findAllByRole("dialog", { name: "L'hôte a arrêté le serveur" })).toHaveLength(1);
  });
});
