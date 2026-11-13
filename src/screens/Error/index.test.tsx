import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import { DEFAULT_ERROR_STATE, ERROR_STATES, STATES } from "./states";

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
    expect(screen.getByRole(STATES[id].wait ? "status" : "alert")).toBeTruthy();
  });

  it("état inconnu ou absent : état par défaut", () => {
    const a = show("#/error?state=zzz");
    expect(a.container.querySelector(`[data-state="${DEFAULT_ERROR_STATE}"]`)).toBeTruthy();
    cleanup();
    const b = show("#/error");
    expect(b.container.querySelector(`[data-state="${DEFAULT_ERROR_STATE}"]`)).toBeTruthy();
  });

  it("Réessayer / Annuler / Installer Java naviguent", () => {
    show("#/error?state=disk-full");
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(window.location.hash).toBe("#/main");
    cleanup();
    show("#/error?state=java-missing");
    fireEvent.click(screen.getByRole("button", { name: /Installer Java/ }));
    expect(window.location.hash).toBe("#/install");
    cleanup();
    show("#/error?state=no-license");
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(window.location.hash).toBe("#/login");
  });

  it("lien Obtenir Minecraft externe", () => {
    show("#/error?state=no-license");
    expect(screen.getByRole("link", { name: "Obtenir Minecraft" }).getAttribute("href")).toBe("https://www.minecraft.net/");
  });

  it("Rouvrir la fenêtre appelle le service puis Annuler revient à la connexion", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const spy = vi.spyOn(svc, "authBeginLogin");
    show("#/error?state=waiting", svc);
    fireEvent.click(screen.getByRole("button", { name: "Rouvrir la fenêtre" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(window.location.hash).toBe("#/login");
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

  it("lancement échoué : détail technique copiable, Réessayer relance l'instance", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const launch = vi.fn(async () => {});
    show("#/error?state=launch-failed&instance=i-1&detail=Code%20de%20sortie%20%3A%201", { ...base, instancesLaunch: launch });
    expect(screen.getByRole("heading", { level: 1, name: "Minecraft s'est arrêté avant de démarrer" })).toBeTruthy();
    expect(screen.getByText("Code de sortie : 1")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Copier le détail" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Détail technique/ }));
    expect(screen.queryByText("Code de sortie : 1")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    await waitFor(() => expect(launch).toHaveBeenCalledWith("i-1"));
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
  });

  it("sans données : libellés neutres, aucune valeur inventée", () => {
    show("#/error?state=disk-full");
    expect(document.body.textContent).not.toMatch(/Alex|1,2 Go/);
    cleanup();
    show("#/error?state=bad-hash");
    expect(document.body.textContent).not.toMatch(/Alex|aeronautics|2 autres/);
  });
});
