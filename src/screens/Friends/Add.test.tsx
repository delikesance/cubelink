import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ShellContext } from "../../app/context";
import { offerFriendLink, resetFriendLink } from "../../app/friendLink";
import { resetOutgoing, useOutgoingIntake } from "../../app/outgoingRequest";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services/types";
import Add, { validateFriendCode } from "./Add";

afterEach(() => { cleanup(); vi.restoreAllMocks(); resetFriendLink(); resetOutgoing(); });

const VALID = "mcl1-0123456789abcdef0123";
let fake: ReturnType<typeof createFakeServices>;

let services: Services;
let unmountAdd: () => void = () => {};

function mount() {
  const r = render(
    <ShellContext.Provider value={{ services, route: "friends", params: {}, navigate: vi.fn(), setTitle: vi.fn(), goBack: vi.fn(), canGoBack: false } as never}>
      <Add />
    </ShellContext.Provider>,
  );
  unmountAdd = r.unmount;
}

function setup(over: Partial<Services> = {}) {
  const base = createFakeServices({ tickMs: 0 });
  fake = base;
  const send = vi.fn(over.friendSendRequest ?? base.friendSendRequest);
  services = { ...base, ...over, friendSendRequest: send } as Services;
  mount();
  return send;
}
const input = () => screen.getByLabelText("Code ami de ton ami") as HTMLInputElement;
const sendBtn = () => screen.getByRole("button", { name: "Envoyer la demande" }) as HTMLButtonElement;

describe("validateFriendCode", () => {
  it("règles", () => {
    expect(validateFriendCode(`  ${VALID}\n`)).toBeNull();
    expect(validateFriendCode(VALID.toUpperCase())).toBeNull();
    expect(validateFriendCode("launcher://add/abc")).toBeNull();
    expect(validateFriendCode("cubelink://add/abc")).toBeNull();
    expect(validateFriendCode("CubeLink://add/abc/")).toBeNull();
    expect(validateFriendCode("cubelink://autre/abc")).not.toBeNull();
    expect(validateFriendCode("https://add/abc")).not.toBeNull();
    expect(validateFriendCode("mcl1-court")).not.toBeNull();
    expect(validateFriendCode("n'importe quoi")).not.toBeNull();
  });
});

describe("onglet Ajouter un ami", () => {
  it("bouton désactivé quand le champ est vide", () => {
    setup();
    expect(sendBtn().disabled).toBe(true);
    fireEvent.change(input(), { target: { value: "  " } });
    expect(sendBtn().disabled).toBe(true);
  });

  it("code valide : envoi, code de vérification affiché, champ vidé", async () => {
    const send = setup();
    fireEvent.change(input(), { target: { value: `  ${VALID}\n` } });
    fireEvent.click(sendBtn());
    await screen.findAllByText(/Demande envoyée\. Compare ce code avec celui de ton ami/);
    expect(send).toHaveBeenCalledWith(VALID);
    expect(screen.getByLabelText(/Code de vérification : [0-9A-F]{4} · /)).toBeTruthy();
    expect(input().value).toBe("");
  });

  it("code invalide : erreur liée au champ, aucun appel au service", async () => {
    const send = setup();
    fireEvent.change(input(), { target: { value: "mcl1-court" } });
    fireEvent.click(sendBtn());
    const err = await screen.findByText(/n'est pas reconnu/);
    expect(input().getAttribute("aria-describedby")).toContain(err.id);
    expect(input().getAttribute("aria-invalid")).toBe("true");
    expect(send).not.toHaveBeenCalled();
  });

  it("lien launcher://add/…", async () => {
    const send = setup({ friendSendRequest: async () => "AAAA · BBBB · CCCC · DDDD" });
    fireEvent.change(input(), { target: { value: "launcher://add/xyz123" } });
    fireEvent.click(sendBtn());
    await screen.findByLabelText("Code de vérification : AAAA · BBBB · CCCC · DDDD");
    expect(send).toHaveBeenCalledWith("launcher://add/xyz123");
  });

  it("lien cubelink://add/… collé : envoyé tel quel", async () => {
    const send = setup({ friendSendRequest: async () => "AAAA · BBBB · CCCC · DDDD" });
    fireEvent.change(input(), { target: { value: `cubelink://add/${VALID}` } });
    fireEvent.click(sendBtn());
    await screen.findByLabelText("Code de vérification : AAAA · BBBB · CCCC · DDDD");
    expect(send).toHaveBeenCalledWith(`cubelink://add/${VALID}`);
  });

  describe("lien ouvert depuis le système", () => {
    it("préremplit le champ sans rien envoyer, et le lien n'est repris qu'une fois", async () => {
      const send = setup();
      expect(input().value).toBe("");
      act(() => offerFriendLink(`cubelink://add/${VALID}`));
      await waitFor(() => expect(input().value).toBe(`cubelink://add/${VALID}`));
      expect(document.activeElement).toBe(input());
      expect(screen.getAllByText(/rien n'est envoyé/i).length).toBeGreaterThan(0);
      expect(sendBtn().disabled).toBe(false);
      expect(screen.queryByRole("alert")).toBeNull();
      expect(send).not.toHaveBeenCalled();
      // Un geste de l'utilisateur envoie la demande, avec le code de vérification habituel.
      fireEvent.click(sendBtn());
      await screen.findByLabelText(/Code de vérification/);
      expect(send).toHaveBeenCalledTimes(1);
      expect(send).toHaveBeenCalledWith(`cubelink://add/${VALID}`);
    });

    it("lien présent avant l'affichage de l'écran : repris au montage", async () => {
      offerFriendLink(`cubelink://add/${VALID}`);
      setup();
      await waitFor(() => expect(input().value).toBe(`cubelink://add/${VALID}`));
    });

    it("lien mal formé : il remplit le champ, aucune demande ne part tant qu'on n'envoie pas, puis l'erreur du service s'affiche", async () => {
      const send = setup();
      act(() => offerFriendLink("cubelink://add/x"));
      await waitFor(() => expect(input().value).toBe("cubelink://add/x"));
      expect(send).not.toHaveBeenCalled();
      fireEvent.click(sendBtn());
      expect((await screen.findByRole("alert")).textContent).toMatch(/Code ami invalide/);
    });

    it("lien d'un autre schéma : refusé par le champ avant tout appel", async () => {
      const send = setup();
      act(() => offerFriendLink("cubelink://autre/x"));
      await waitFor(() => expect(input().value).toBe("cubelink://autre/x"));
      fireEvent.click(sendBtn());
      await screen.findByText(/n'est pas reconnu/);
      expect(send).not.toHaveBeenCalled();
    });

    it("pendant une demande en attente de réponse : l'écran n'est pas écrasé, le lien est repris après l'annulation", async () => {
      setup();
      fireEvent.change(input(), { target: { value: VALID } });
      fireEvent.click(sendBtn());
      await screen.findByLabelText(/Code de vérification/);
      act(() => offerFriendLink("cubelink://add/mcl1-ffffffffffffffffffff"));
      expect(screen.getByText(/Un lien d'ami est arrivé/)).toBeTruthy();
      expect(screen.getByLabelText(/Code de vérification/)).toBeTruthy();
      expect(input().value).toBe("");
      fireEvent.click(screen.getByRole("button", { name: "Annuler la demande" }));
      await waitFor(() => expect(input().value).toBe("cubelink://add/mcl1-ffffffffffffffffffff"));
      expect(screen.queryByText(/Un lien d'ami est arrivé/)).toBeNull();
    });
  });

  it("erreur du service affichée en alerte, champ conservé", async () => {
    setup({ friendSendRequest: async () => { throw new Error("Ce code est déjà dans ta liste d'amis."); } });
    fireEvent.change(input(), { target: { value: VALID } });
    fireEvent.click(sendBtn());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/déjà dans ta liste/);
    await waitFor(() => expect(input().value).toBe(VALID));
  });

  it("Coller : lit le presse-papiers, repli silencieux sinon", async () => {
    setup();
    Object.defineProperty(navigator, "clipboard", { value: { readText: async () => ` ${VALID} ` }, configurable: true });
    fireEvent.click(screen.getByRole("button", { name: "Coller" }));
    await waitFor(() => expect(input().value).toBe(VALID));
    Object.defineProperty(navigator, "clipboard", { value: { readText: async () => { throw new Error("refusé"); } }, configurable: true });
    fireEvent.click(screen.getByRole("button", { name: "Coller" }));
    await waitFor(() => expect(document.activeElement).toBe(input()));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  async function sendValid() {
    fireEvent.change(input(), { target: { value: VALID } });
    fireEvent.click(sendBtn());
    await screen.findByLabelText(/Code de vérification/);
  }

  it("Annuler la demande : le service est appelé, retour au formulaire, renvoi immédiat possible", async () => {
    setup();
    await sendValid();
    fireEvent.click(screen.getByRole("button", { name: "Annuler la demande" }));
    await waitFor(() => expect(screen.queryByLabelText(/Code de vérification/)).toBeNull());
    expect(input().disabled).toBe(false);
    // Le pair est libéré : pas de « déjà en cours ».
    fireEvent.change(input(), { target: { value: VALID } });
    fireEvent.click(sendBtn());
    await screen.findByLabelText(/Code de vérification/);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("pendant qu'une demande attend, on ne peut pas en envoyer une autre (une seule est suivie)", async () => {
    const send = setup();
    await sendValid();
    fireEvent.change(input(), { target: { value: "mcl1-fedcba98765432100123" } });
    expect(sendBtn().disabled).toBe(true);
    fireEvent.submit(input().closest("form")!);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("refus reçu hors de l'écran : retrouvé au retour, sans bouton Annuler ni blocage du renvoi", async () => {
    setup();
    await sendValid();
    unmountAdd();
    renderHook(() => useOutgoingIntake(services)); // le Shell écoute pendant que l'écran est fermé
    act(() => fake.answerSentRequest(VALID, "refused"));
    mount();
    expect((await screen.findAllByText("Ta demande n'a pas été acceptée.")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: "Annuler la demande" })).toBeNull();
  });

  it("demande en attente au retour sur l'écran : le code de vérification et « Annuler » sont toujours là", async () => {
    setup();
    await sendValid();
    unmountAdd();
    mount();
    expect(screen.getByLabelText(/Code de vérification/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Annuler la demande" })).toBeTruthy();
  });

  it("acceptée : le nom de l'ami est annoncé", async () => {
    setup();
    await sendValid();
    act(() => fake.answerSentRequest(VALID, "accepted", "Camille"));
    expect((await screen.findAllByText("Camille a accepté ta demande. Vous êtes amis.")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: "Annuler la demande" })).toBeNull();
  });

  it("refusée : message neutre, sans reproche", async () => {
    setup();
    await sendValid();
    act(() => fake.answerSentRequest(VALID, "refused"));
    expect((await screen.findAllByText("Ta demande n'a pas été acceptée.")).length).toBeGreaterThan(0);
  });

  it("sans réponse : le dire et proposer de redemander", async () => {
    setup();
    await sendValid();
    act(() => fake.answerSentRequest(VALID, "expired"));
    expect((await screen.findAllByText(/restée sans réponse/)).length).toBeGreaterThan(0);
  });

  it("une demande annulée ne reçoit plus de réponse", async () => {
    setup();
    await sendValid();
    fireEvent.click(screen.getByRole("button", { name: "Annuler la demande" }));
    await waitFor(() => expect(screen.queryByLabelText(/Code de vérification/)).toBeNull());
    act(() => fake.answerSentRequest(VALID, "refused"));
    expect(screen.queryByText("Ta demande n'a pas été acceptée.")).toBeNull();
  });
});
