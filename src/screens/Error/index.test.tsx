import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import { buildStates, DEFAULT_ERROR_STATE, ERROR_STATES, STATES } from "./states";

afterEach(() => { cleanup(); window.location.hash = ""; });

const show = (hash: string, svc = createFakeServices({ tickMs: 0 })) => {
  window.location.hash = hash;
  return render(<Shell services={svc} />);
};

describe("écran Erreur", () => {
  it.each(ERROR_STATES)("état %s : titre, rôle, focus", async (id) => {
    const { container } = show(`#/error?state=${id}`);
    expect(container.querySelector(`[data-state="${id}"]`)).toBeTruthy();
    const h = await screen.findByRole("heading", { level: 1, name: STATES[id].title });
    await waitFor(() => expect(document.activeElement).toBe(h));
    expect(screen.getByRole("alert")).toBeTruthy();
  });

  it("état inconnu ou absent : état par défaut", () => {
    const a = show("#/error?state=zzz");
    expect(a.container.querySelector(`[data-state="${DEFAULT_ERROR_STATE}"]`)).toBeTruthy();
    cleanup();
    const b = show("#/error");
    expect(b.container.querySelector(`[data-state="${DEFAULT_ERROR_STATE}"]`)).toBeTruthy();
  });

  it("Réessayer / Annuler naviguent", () => {
    show("#/error?state=disk-full");
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(window.location.hash).toBe("#/main");
    cleanup();
    show("#/error?state=no-license");
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(window.location.hash).toBe("#/login");
  });

  it("lien Obtenir Minecraft externe", () => {
    show("#/error?state=no-license");
    expect(screen.getByRole("link", { name: "Obtenir Minecraft" }).getAttribute("href")).toBe("https://www.minecraft.net/");
  });

  it("Réessayer (bad-hash) avec un ami retourne à l'écran Rejoindre, sans appel de service", () => {
    const base = createFakeServices({ tickMs: 0 });
    const start = vi.fn(async () => {});
    show("#/error?state=bad-hash&friend=pk-1&name=Léa&file=a.jar&others=2", { ...base, joinStart: start });
    expect(document.getElementById("err-title")?.textContent).toContain("Léa");
    expect(screen.getByText("a.jar")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(window.location.hash).toBe("#/join?friend=pk-1");
    expect(start).not.toHaveBeenCalled();
  });

  it("Réessayer (bad-hash) sans ami retombe sur l'accueil", () => {
    show("#/error?state=bad-hash");
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(window.location.hash).toBe("#/main");
  });

  it.each(["java-missing", "waiting", "launch-failed"])("état supprimé %s : retombe sur l'état par défaut", (id) => {
    const { container } = show(`#/error?state=${id}`);
    expect(container.querySelector(`[data-state="${DEFAULT_ERROR_STATE}"]`)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Java|Rouvrir|Copier le détail/ })).toBeNull();
  });

  it("aucun texte de maquette : ni prénom en dur (Alex), ni durée de 5 minutes, avec ou sans contexte", () => {
    const texts = [buildStates(), buildStates({ friend: "Léa", file: "a.jar", others: 2, need: "1,2 Go" })].flatMap((all) =>
      Object.values(all).map((d) => {
        const { container } = render(<>{d.kicker} {d.title} {d.body} {d.more}</>);
        const t = container.textContent ?? "";
        cleanup();
        return t;
      }));
    expect(texts.length).toBe(ERROR_STATES.length * 2);
    for (const t of texts) expect(t).not.toMatch(/Alex|5 minutes|Java 21/);
  });

  it("sans données : libellés neutres, aucune valeur inventée", () => {
    show("#/error?state=disk-full");
    expect(document.body.textContent).not.toMatch(/Alex|1,2 Go/);
    cleanup();
    show("#/error?state=bad-hash");
    expect(document.body.textContent).not.toMatch(/Alex|aeronautics|2 autres/);
  });
});
