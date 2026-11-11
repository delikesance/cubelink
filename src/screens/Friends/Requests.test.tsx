import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ShellContext } from "../../app/context";
import { navigate } from "../../app/router";
import { createFakeServices } from "../../services/fake";
import type { FriendRequest, Services } from "../../services/types";
import Requests, { badgeLabel } from "./Requests";

afterEach(cleanup);

function mount(services: Services) {
  return render(
    <ShellContext.Provider value={{ services, route: "friends" as never, params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false }}>
      <Requests />
    </ShellContext.Provider>,
  );
}
const fake = (n = 1000) => createFakeServices({ tickMs: 0, requestCount: n });

describe("Demandes", () => {
  it("affiche la première page avec une seule demande dépliée", async () => {
    mount(fake());
    expect(await screen.findByRole("heading", { level: 3, name: "Demande de Alex0" })).toBeTruthy();
    expect(screen.getAllByRole("heading", { level: 3 })).toHaveLength(1);
    expect(screen.getAllByLabelText("Code de vérification")).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: /^Voir la demande de/ })).toHaveLength(19);
    expect(screen.getByRole("button", { name: "Voir les 980 autres demandes" })).toBeTruthy();
    expect(screen.getByText("99+")).toBeTruthy();
  });

  it("badge : 99+ au-delà de 99", () => {
    expect(badgeLabel(100)).toBe("99+");
    expect(badgeLabel(7)).toBe("7");
    expect(badgeLabel(0)).toBe("");
  });

  it("Voir déplie une autre demande et replie la première", async () => {
    mount(fake());
    fireEvent.click(await screen.findByRole("button", { name: "Voir la demande de Steve1" }));
    expect(screen.getByRole("heading", { level: 3, name: "Demande de Steve1" })).toBeTruthy();
    expect(screen.getAllByRole("heading", { level: 3 })).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Voir la demande de Alex0" })).toBeTruthy();
  });

  it("accepte puis refuse : ligne retirée, total mis à jour, suivante dépliée", async () => {
    const svc = fake();
    mount(svc);
    fireEvent.click(await screen.findByLabelText("J'ai comparé ce code avec Alex0"));
    fireEvent.click(await screen.findByRole("button", { name: "Accepter Alex0" }));
    expect(await screen.findByRole("heading", { level: 3, name: "Demande de Steve1" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Voir les 980 autres demandes" })).toBeTruthy();
    expect((await svc.friendList()).some((f) => f.friend.name === "Alex0")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Refuser Steve1" }));
    expect(await screen.findByRole("heading", { level: 3, name: "Demande de Noor2" })).toBeTruthy();
    expect((await svc.friendRequests({ offset: 0, limit: 1 })).total).toBe(998);
    expect(screen.queryByRole("button", { name: "Voir la demande de Steve1" })).toBeNull();
  });

  it("recherche par nom", async () => {
    mount(fake());
    await screen.findByText("Demande de Alex0");
    fireEvent.change(screen.getByLabelText("Chercher une demande"), { target: { value: "Inès" } });
    await waitFor(() => expect(screen.getByRole("heading", { level: 3, name: "Demande de Inès6" })).toBeTruthy());
    expect(screen.queryByRole("button", { name: "Voir la demande de Alex0" })).toBeNull();
    fireEvent.change(screen.getByLabelText("Chercher une demande"), { target: { value: "zzzz" } });
    expect(await screen.findByText("Aucune demande ne correspond à cette recherche.")).toBeTruthy();
  });

  it("tout refuser demande confirmation et peut être annulé", async () => {
    const svc = fake(30);
    mount(svc);
    await screen.findByText("Demande de Alex0");
    fireEvent.click(screen.getByRole("button", { name: "Tout refuser" }));
    expect(screen.getByText(/Refuser les 30 demandes/, { selector: "p" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(screen.queryByRole("group")).toBeNull();
    expect((await svc.friendRequests({ offset: 0, limit: 1 })).total).toBe(30);
    fireEvent.click(screen.getByRole("button", { name: "Tout refuser" }));
    const group = screen.getByRole("group");
    fireEvent.click(within(group).getByRole("button", { name: "Refuser les 30 demandes" }));
    expect(await screen.findByText("Aucune demande en attente.")).toBeTruthy();
    expect((await svc.friendRequests({ offset: 0, limit: 1 })).total).toBe(0);
  });

  it("pagination par lots", async () => {
    mount(fake(45));
    fireEvent.click(await screen.findByRole("button", { name: "Voir les 25 autres demandes" }));
    expect(await screen.findByRole("button", { name: "Voir les 5 autres demandes" })).toBeTruthy();
    expect(screen.getAllByRole("button", { name: /^Voir la demande de/ })).toHaveLength(39);
    fireEvent.click(screen.getByRole("button", { name: "Voir les 5 autres demandes" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: /autres demandes/ })).toBeNull());
    expect(screen.getAllByRole("button", { name: /^Voir la demande de/ })).toHaveLength(44);
  });

  it("annonce une nouvelle demande reçue", async () => {
    const base = fake(3);
    let push: (r: FriendRequest) => void = () => {};
    const svc: Services = { ...base, onFriendRequest: (cb) => { push = cb; return () => {}; } };
    mount(svc);
    await screen.findByText("Demande de Alex0");
    act(() => push({ public_key: "ff", name: "Nouveau", verify_code: "AAAA · BBBB · CCCC · DDDD", received_at: 1 }));
    expect(screen.getByText("Nouvelle demande de Nouveau.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Voir la demande de Nouveau" })).toBeTruthy();
  });

  it("reste fluide avec 1 000 demandes : seules les lignes chargées sont rendues", async () => {
    mount(fake(1000));
    await screen.findByText("Demande de Alex0");
    expect(document.querySelectorAll(".rq-item").length).toBeLessThanOrEqual(20);
  });

  it("état vide", async () => {
    mount(fake(0));
    expect(await screen.findByText("Aucune demande en attente.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Tout refuser" })).toBeNull();
  });

  it("Accepter reste désactivé tant que le code n'est pas coché comme comparé", async () => {
    const svc = fake();
    mount(svc);
    const accept = (await screen.findByRole("button", { name: "Accepter Alex0" })) as HTMLButtonElement;
    expect(accept.disabled).toBe(true);
    fireEvent.click(screen.getByLabelText("J'ai comparé ce code avec Alex0"));
    expect(accept.disabled).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Voir la demande de Steve1" }));
    expect((screen.getByRole("button", { name: "Accepter Steve1" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
