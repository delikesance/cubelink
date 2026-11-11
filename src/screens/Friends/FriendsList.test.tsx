import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ShellContext } from "../../app/context";
import { navigate } from "../../app/router";
import { createFakeServices } from "../../services/fake";
import FriendsList from "./FriendsList";

afterEach(cleanup);

describe("Tes amis", () => {
  it("affiche les amis avec un bouton Rejoindre étiqueté ; état vide sinon", async () => {
    const services = createFakeServices({ tickMs: 0 });
    render(
      <ShellContext.Provider value={{ services, route: "friends" as never, params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false }}>
        <FriendsList />
      </ShellContext.Provider>,
    );
    const list = await services.friendList();
    if (list.length === 0) {
      expect(await screen.findByText(/Aucun ami pour l'instant/)).toBeTruthy();
    } else {
      expect(await screen.findByRole("button", { name: `Rejoindre ${list[0]!.friend.name}` })).toBeTruthy();
    }
  });
});
