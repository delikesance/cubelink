// Garde-fou des E2E Windows (ticket #66) : la table `e2e/harness/routes.ts` et les sélecteurs des specs doivent rester
// d'accord avec le routeur et les écrans réels. Les E2E ne tournent que sous Windows, cette dérive passait inaperçue.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ROUTE_ALIASES, routeOf } from "../../e2e/harness/routes";
import { createFakeServices } from "../services/fake";
import { parseHash } from "./router";
import { Shell } from "./shell";

afterEach(() => { cleanup(); window.location.hash = ""; });

describe("table des routes des E2E", () => {
  it("chaque alias donne, via le routeur, la route attendue (data-route)", () => {
    for (const [name, def] of Object.entries(ROUTE_ALIASES)) {
      expect(parseHash(def.hash).route, name).toBe(def.route);
    }
    // l'ancien écran Amis n'est plus une route : il vit dans Paramètres > Amis
    expect(parseHash("#/friends")).toMatchObject({ route: "settings", params: { tab: "amis" } });
    expect(ROUTE_ALIASES.friends.route).toBe("settings");
  });

  it("routeOf : alias, route simple, route avec paramètres", () => {
    expect(routeOf("friends")).toBe(ROUTE_ALIASES.friends);
    expect(routeOf("login")).toEqual({ hash: "#/login", route: "login" });
    expect(routeOf("join?friend=ab")).toEqual({ hash: "#/join?friend=ab", route: "join" });
  });
});

describe("sélecteurs « prêt » et locators des specs E2E sur les vrais écrans", () => {
  it("Amis : data-route=settings, champ d'ajout, code ami et demandes reçues, sans onglet « Demandes »", async () => {
    window.location.hash = ROUTE_ALIASES.friends.hash;
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    await waitFor(() => expect(document.querySelector(`main[data-route="${ROUTE_ALIASES.friends.route}"]`)).not.toBeNull());
    await waitFor(() => expect(document.querySelector(ROUTE_ALIASES.friends.ready!)).not.toBeNull());
    expect(await screen.findByLabelText("Code ami de ton ami")).toBeTruthy();
    expect(await screen.findByLabelText(/^Mon code ami : /)).toBeTruthy();
    expect(screen.getByRole("heading", { name: /Demandes reçues/ })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Amis" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.queryByRole("tab", { name: /Demandes/ })).toBeNull();
  });

  it("Accueil : tuile prête, colonne d'amis, partie en cours (Rejoindre + « Joue à ») au lieu de l'ancienne liste d'amis", async () => {
    window.location.hash = ROUTE_ALIASES.main.hash;
    const svc = createFakeServices({ tickMs: 0 });
    render(<Shell services={svc} />);
    await waitFor(() => expect(document.querySelector(`main[data-route="${ROUTE_ALIASES.main.route}"]`)).not.toBeNull());
    await waitFor(() => expect(document.querySelector(ROUTE_ALIASES.main.ready!)).not.toBeNull());
    const side = screen.getByRole("complementary", { name: "Amis" });
    const alex = (await svc.friendList())[0].friend;
    await within(side).findByText(alex.name);
    expect(screen.queryByRole("list", { name: "Parties en cours" })).toBeNull();
    // demandes en attente : lien « N demande(s) » (01-add-friend l'attend, avec N = 1), plus de bouton « Amis (N demande) »
    expect(await screen.findByRole("button", { name: /^(\d+|99\+) demandes?$/ })).toBeTruthy();
    act(() => { svc.emitInvite({ public_key: alex.public_key }); });
    const parties = await screen.findByRole("list", { name: "Parties en cours" });
    expect(within(parties).getByRole("button", { name: `Rejoindre ${alex.name}` })).toBeTruthy();
    // la colonne d'amis a un bouton du même nom : les specs ciblent la liste des parties pour éviter l'ambiguïté
    expect(screen.getAllByRole("button", { name: `Rejoindre ${alex.name}` }).length).toBeGreaterThan(1);
    expect(within(side).getByText(/^Joue à /)).toBeTruthy();
  });

  it("Parcourir : la fiche est une page (Version = Select, Retour aux résultats, Source, lien vers le projet), pas un dialogue", async () => {
    window.location.hash = "#/browse?kind=mod";
    const svc = createFakeServices({ tickMs: 0 });
    const { container } = render(<Shell services={svc} />);
    const card = await waitFor(() => {
      const b = container.querySelector<HTMLElement>(".br-card-btn");
      expect(b).not.toBeNull();
      return b!;
    });
    fireEvent.click(card);
    const version = await screen.findByRole("combobox", { name: "Version" });
    fireEvent.click(version);
    expect(within(screen.getByRole("listbox")).getAllByRole("option").length).toBeGreaterThan(0);
    fireEvent.click(version);
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByText(/Source : /)).toBeTruthy();
    expect(screen.getByRole("link", { name: /Ouvrir la page sur / })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Installer/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Retour aux résultats" }));
    await waitFor(() => expect(container.querySelector(".br-card-btn")).not.toBeNull());
  });

  it("Page du modpack : menu « Actions de … » (Chercher automatiquement) et onglet Mods en panneau (pas un dialogue)", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const inst = (await svc.instancesList())[0];
    window.location.hash = `#/instance?id=${inst.id}&tab=mods`;
    const { container } = render(<Shell services={svc} />);
    await waitFor(() => expect(container.querySelector(".dr-panel")).not.toBeNull());
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: `Actions de ${inst.name}` }));
    expect(await screen.findByRole("menuitem", { name: "Chercher automatiquement" })).toBeTruthy();
  });
});
