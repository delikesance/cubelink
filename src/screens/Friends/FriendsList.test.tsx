import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
});
