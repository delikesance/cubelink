import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { navigate, goBack } from "../../app/router";
import { createFakeServices } from "../../services/fake";
import InGamePanel from ".";

afterEach(cleanup);

const mount = (svc: ReturnType<typeof createFakeServices>) => {
  const ctx: ShellContextValue = { services: svc, route: "main", params: {}, navigate, goBack, canGoBack: false, setTitle: () => {} };
  render(<ShellContext.Provider value={ctx}><InGamePanel /></ShellContext.Provider>);
};

describe("Panneau en jeu : modpack en cours", () => {
  it("le modpack lancé a un bouton rouge Fermer qui ferme le jeu après confirmation ; les autres gardent Lancer", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await svc.instancesLaunch("inst-atm10");
    mount(svc);
    const close = await screen.findByRole("button", { name: "Fermer All the Mods 10" });
    expect(close.className).toContain("cl-btn-danger");
    expect(screen.queryByRole("button", { name: "Lancer All the Mods 10" })).toBeNull();
    expect(screen.getByRole("button", { name: "Lancer Create: Aeronautics" })).toBeTruthy();

    fireEvent.click(close);
    expect(svc.gameCalls).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Fermer le jeu" }));
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "close", arg: "inst-atm10" }]));
    // le jeu terminé : le bouton redevient Lancer
    expect(await screen.findByRole("button", { name: "Lancer All the Mods 10" })).toBeTruthy();
  });

  it("sans jeu en cours : uniquement des boutons Lancer", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    mount(svc);
    await screen.findByRole("button", { name: "Lancer All the Mods 10" });
    expect(screen.queryByRole("button", { name: "Fermer All the Mods 10" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Fermer Create: Aeronautics" })).toBeNull();
  });
});
