import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ShellContext } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services/types";
import Add, { validateFriendCode } from "./Add";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const VALID = "mcl1-0123456789abcdef0123";

function setup(over: Partial<Services> = {}) {
  const base = createFakeServices({ tickMs: 0 });
  const send = vi.fn(over.friendSendRequest ?? base.friendSendRequest);
  const services = { ...base, ...over, friendSendRequest: send } as Services;
  render(
    <ShellContext.Provider value={{ services, route: "friends", params: {}, navigate: vi.fn(), setTitle: vi.fn() } as never}>
      <Add />
    </ShellContext.Provider>,
  );
  return send;
}
const input = () => screen.getByLabelText("Code ami de ton ami") as HTMLInputElement;
const sendBtn = () => screen.getByRole("button", { name: "Envoyer la demande" }) as HTMLButtonElement;

describe("validateFriendCode", () => {
  it("règles", () => {
    expect(validateFriendCode(`  ${VALID}\n`)).toBeNull();
    expect(validateFriendCode(VALID.toUpperCase())).toBeNull();
    expect(validateFriendCode("launcher://add/abc")).toBeNull();
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
});
