import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ShellContext } from "../../app/context";
import { navigate } from "../../app/router";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services/types";
import FriendsList from "./FriendsList";

afterEach(cleanup);

function mount(services: Services) {
  return render(
    <ShellContext.Provider value={{ services, route: "friends" as never, params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false }}>
      <FriendsList />
    </ShellContext.Provider>,
  );
}
const fake = () => createFakeServices({ tickMs: 0, requestCount: 0 });

describe("Mes amis", () => {
  it("une demande envoyée acceptée ajoute le nouvel ami sans recharger l'écran", async () => {
    const services = fake();
    const code = "mcl1-0123456789abcdef0123";
    mount(services);
    await screen.findByRole("list");
    await services.friendSendRequest(code);
    act(() => services.answerSentRequest(code, "accepted", "Camille"));
    expect(await screen.findByText("Camille")).toBeTruthy();
  });

  it("liste les amis, Retirer demande confirmation puis appelle friendRemove et vide la ligne", async () => {
    const services = fake();
    const friendRemove = vi.spyOn(services, "friendRemove");
    const [first] = await services.friendList();
    mount(services);
    fireEvent.click(await screen.findByRole("button", { name: `Retirer ${first.friend.name} de mes amis` }));
    const dialog = screen.getByRole("alertdialog", { name: "Retirer cet ami ?" });
    expect(dialog.textContent).toContain("groupe Hearth");
    expect(friendRemove).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Retirer" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(friendRemove).toHaveBeenCalledWith(first.friend.public_key);
    expect(screen.queryByRole("button", { name: `Retirer ${first.friend.name} de mes amis` })).toBeNull();
    expect(screen.getByRole("status").textContent).toContain(`${first.friend.name} n'est plus dans tes amis.`);
    expect((await services.friendList()).some((f) => f.friend.public_key === first.friend.public_key)).toBe(false);
  });

  it("Annuler et Échap ne retirent personne", async () => {
    const services = fake();
    const friendRemove = vi.spyOn(services, "friendRemove");
    const [first] = await services.friendList();
    mount(services);
    const open = async () => fireEvent.click(await screen.findByRole("button", { name: `Retirer ${first.friend.name} de mes amis` }));
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await open();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(friendRemove).not.toHaveBeenCalled();
  });

  it("erreur du retrait : affichée dans la boîte, l'ami reste", async () => {
    const services = { ...fake(), friendRemove: async () => { throw new Error("Impossible d'écrire la liste d'amis."); } };
    const [first] = await services.friendList();
    mount(services);
    fireEvent.click(await screen.findByRole("button", { name: `Retirer ${first.friend.name} de mes amis` }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Retirer" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Impossible d'écrire");
    expect(screen.getByRole("alertdialog")).toBeTruthy();
  });

  it("aucun ami : message vide", async () => {
    const services = { ...fake(), friendList: async () => [] };
    mount(services);
    expect((await screen.findByText(/pas encore d'ami/)).textContent).toContain("Ajoute-en un");
  });

  it("affiche l'état initial de confiance pour chaque ami (Alex non coché, Noor coché)", async () => {
    const services = fake();
    mount(services);
    const alexBox = (await screen.findByRole("checkbox", { name: "Installer les mods de Alex sans me demander" })) as HTMLInputElement;
    const noorBox = screen.getByRole("checkbox", { name: "Installer les mods de Noor sans me demander" }) as HTMLInputElement;
    expect(alexBox.checked).toBe(false);
    expect(noorBox.checked).toBe(true);
  });

  it("cocher/décocher la confiance appelle friendSetTrust, met à jour la case, annonce et persiste", async () => {
    const services = fake();
    const spy = vi.spyOn(services, "friendSetTrust");
    const [alex] = await services.friendList();
    mount(services);
    const alexBox = (await screen.findByRole("checkbox", { name: `Installer les mods de ${alex.friend.name} sans me demander` })) as HTMLInputElement;
    fireEvent.click(alexBox);
    await waitFor(() => expect(alexBox.checked).toBe(true));
    expect(spy).toHaveBeenCalledWith(alex.friend.public_key, "always");
    expect(screen.getByRole("status").textContent).toContain("sans te demander");
    expect((await services.friendList())[0].friend.trust).toBe("always");

    fireEvent.click(alexBox);
    await waitFor(() => expect(alexBox.checked).toBe(false));
    expect(spy).toHaveBeenCalledWith(alex.friend.public_key, "ask");
    expect(screen.getByRole("status").textContent).toContain("demandera à nouveau");
    expect((await services.friendList())[0].friend.trust).toBe("ask");
  });

  it("erreur de friendSetTrust : alerte affichée et état précédent conservé", async () => {
    const services = { ...fake(), friendSetTrust: async () => { throw new Error("Impossible d'enregistrer la confiance."); } };
    mount(services);
    const alexBox = (await screen.findByRole("checkbox", { name: "Installer les mods de Alex sans me demander" })) as HTMLInputElement;
    expect(alexBox.checked).toBe(false);
    fireEvent.click(alexBox);
    expect((await screen.findByRole("alert")).textContent).toContain("Impossible d'enregistrer la confiance.");
    expect(alexBox.checked).toBe(false);
  });
});
