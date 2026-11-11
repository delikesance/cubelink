import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ShellContext } from "../../app/context";
import { navigate } from "../../app/router";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services/types";
import MyCode from "./MyCode";

afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });

function mount(services: Services) {
  return render(
    <ShellContext.Provider value={{ services, route: "friends" as never, params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false }}>
      <MyCode />
    </ShellContext.Provider>,
  );
}

const fake = () => createFakeServices({ tickMs: 0 });

describe("Mon code ami", () => {
  it("affiche le code du service", async () => {
    const svc = fake();
    const expected = await svc.friendMyCode();
    mount(svc);
    expect(screen.getByText("Chargement de ton code…")).toBeTruthy();
    expect(await screen.findByText(expected)).toBeTruthy();
    expect(screen.getByLabelText(`Mon code ami : ${expected}`)).toBeTruthy();
  });

  it("copie dans le presse-papiers puis le message disparaît", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const svc = fake();
    const expected = await svc.friendMyCode();
    mount(svc);
    await screen.findByText(expected);
    vi.useFakeTimers();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Copier" })); });
    expect(writeText).toHaveBeenCalledWith(expected);
    expect(screen.getByText("Copié")).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(2100); });
    expect(screen.queryByText("Copié")).toBeNull();
  });

  it("repli si le presse-papiers est indisponible", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: undefined });
    const svc = fake();
    mount(svc);
    await screen.findByLabelText(/Mon code ami : mcl1-/);
    fireEvent.click(screen.getByRole("button", { name: "Copier" }));
    expect(await screen.findByText(/Copie impossible/)).toBeTruthy();
  });

  it("régénération annulable puis confirmée", async () => {
    const svc = fake();
    const old = await svc.friendMyCode();
    mount(svc);
    await screen.findByText(old);
    fireEvent.click(screen.getByRole("button", { name: "Générer un nouveau code" }));
    expect(screen.getByText(/ancien code ne fonctionnera plus/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(screen.queryByText(/ancien code ne fonctionnera plus/)).toBeNull();
    expect(screen.getByText(old)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Générer un nouveau code" }));
    const group = screen.getByRole("group");
    fireEvent.click(within(group).getByRole("button", { name: "Générer un nouveau code" }));
    await waitFor(() => expect(screen.queryByText(old)).toBeNull());
    expect(screen.getByLabelText(/Mon code ami : mcl1-/)).toBeTruthy();
    expect(screen.queryByRole("group")).toBeNull();
  });

  it("erreur du service puis Réessayer", async () => {
    const base = fake();
    let n = 0;
    const svc: Services = {
      ...base,
      friendMyCode: () => (++n === 1 ? Promise.reject(new Error("Service ami indisponible.")) : base.friendMyCode()),
    };
    mount(svc);
    expect((await screen.findByRole("alert")).textContent).toContain("Service ami indisponible.");
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(await screen.findByLabelText(/Mon code ami : mcl1-/)).toBeTruthy();
  });
});
