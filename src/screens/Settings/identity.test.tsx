import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices, FAKE_IDENTITY_KEY, WRONG_PASSPHRASE } from "../../services/fake";

beforeEach(() => { window.location.hash = "#/settings"; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const NOOR = { id: "noor", name: "Noor", kind: "offline" as const, uuid: "00000000-0000-0000-0000-000000000001" };

async function open() {
  const svc = createFakeServices({ tickMs: 0, loginDelayMs: 0, accounts: [NOOR] });
  render(<Shell services={svc} />);
  const card = await screen.findByRole("region", { name: "Sauvegarde de l'identité" });
  return { svc, card: within(card) };
}
const type = (el: HTMLElement, v: string) => fireEvent.change(el, { target: { value: v } });

describe("Sauvegarde de l'identité", () => {
  it("export : refuse une phrase trop courte et deux phrases différentes, sans rien envoyer", async () => {
    const { svc, card } = await open();
    fireEvent.click(card.getByRole("button", { name: "Sauvegarder mon identité" }));
    expect(card.getByText(/Si tu perds cette phrase de passe, la sauvegarde est perdue/)).toBeTruthy();
    type(card.getByLabelText("Phrase de passe"), "court");
    type(card.getByLabelText("Confirme la phrase de passe"), "court");
    fireEvent.click(card.getByRole("button", { name: /Choisir où enregistrer/ }));
    expect((await card.findByRole("alert")).textContent).toMatch(/8 caractères au minimum/);
    type(card.getByLabelText("Phrase de passe"), "une phrase solide");
    type(card.getByLabelText("Confirme la phrase de passe"), "une phrase solide !");
    fireEvent.click(card.getByRole("button", { name: /Choisir où enregistrer/ }));
    await waitFor(() => expect(card.getByRole("alert").textContent).toMatch(/pas identiques/));
    expect(svc.identityCalls).toEqual([]);
  });

  it("export : enregistre sous l'extension .cubelink-id puis annonce la clé sauvegardée", async () => {
    const { svc, card } = await open();
    svc.setSavePath("/home/noor/ma-sauvegarde");
    fireEvent.click(card.getByRole("button", { name: "Sauvegarder mon identité" }));
    type(card.getByLabelText("Phrase de passe"), "une phrase solide");
    type(card.getByLabelText("Confirme la phrase de passe"), "une phrase solide");
    fireEvent.click(card.getByRole("button", { name: /Choisir où enregistrer/ }));
    await waitFor(() => expect(card.getByText(/Sauvegarde enregistrée pour l'identité abababab…/)).toBeTruthy());
    expect(svc.identityCalls).toEqual([{ kind: "export", path: "/home/noor/ma-sauvegarde.cubelink-id", passphrase: "une phrase solide" }]);
    // Le formulaire (et la phrase) a disparu.
    expect(card.queryByLabelText("Phrase de passe")).toBeNull();
  });

  it("export : sélecteur annulé, rien n'est écrit et aucune erreur", async () => {
    const { svc, card } = await open();
    svc.setSavePath(null);
    fireEvent.click(card.getByRole("button", { name: "Sauvegarder mon identité" }));
    type(card.getByLabelText("Phrase de passe"), "une phrase solide");
    type(card.getByLabelText("Confirme la phrase de passe"), "une phrase solide");
    fireEvent.click(card.getByRole("button", { name: /Choisir où enregistrer/ }));
    await waitFor(() => expect((card.getByRole("button", { name: /Choisir où enregistrer/ }) as HTMLButtonElement).disabled).toBe(false));
    expect(svc.identityCalls).toEqual([]);
    expect(card.queryByRole("alert")).toBeNull();
  });

  it("restauration : mauvaise phrase de passe, message d'erreur et rien à redémarrer", async () => {
    const { svc, card } = await open();
    svc.setPickedFiles(["/home/noor/Cubelink.cubelink-id"]);
    fireEvent.click(card.getByRole("button", { name: "Restaurer une identité" }));
    expect(card.getByText(/remplace ton identité et ta liste d'amis actuelles/)).toBeTruthy();
    type(card.getByLabelText("Phrase de passe de la sauvegarde"), WRONG_PASSPHRASE);
    fireEvent.click(card.getByRole("button", { name: /Choisir la sauvegarde/ }));
    expect((await card.findByRole("alert")).textContent).toBe("Phrase de passe incorrecte, ou fichier abîmé.");
    expect(card.queryByRole("button", { name: "Redémarrer maintenant" })).toBeNull();
  });

  it("restauration : sauvegarde vérifiée puis redémarrage", async () => {
    const { svc, card } = await open();
    svc.setPickedFiles(["/home/noor/Cubelink.cubelink-id"]);
    fireEvent.click(card.getByRole("button", { name: "Restaurer une identité" }));
    type(card.getByLabelText("Phrase de passe de la sauvegarde"), "une phrase solide");
    fireEvent.click(card.getByRole("button", { name: /Choisir la sauvegarde/ }));
    const restart = await card.findByRole("button", { name: "Redémarrer maintenant" });
    expect(card.getByText(new RegExp(`identité ${FAKE_IDENTITY_KEY.slice(0, 8)}…`))).toBeTruthy();
    fireEvent.click(restart);
    await waitFor(() => expect(svc.identityCalls.map((c) => c.kind)).toEqual(["import", "restart"]));
    expect(svc.identityCalls[0]).toMatchObject({ path: "/home/noor/Cubelink.cubelink-id", passphrase: "une phrase solide" });
  });

  it("restauration : « Plus tard » referme le panneau", async () => {
    const { svc, card } = await open();
    svc.setPickedFiles(["/x/a.cubelink-id"]);
    fireEvent.click(card.getByRole("button", { name: "Restaurer une identité" }));
    type(card.getByLabelText("Phrase de passe de la sauvegarde"), "une phrase solide");
    fireEvent.click(card.getByRole("button", { name: /Choisir la sauvegarde/ }));
    fireEvent.click(await card.findByRole("button", { name: "Plus tard" }));
    expect(card.queryByRole("button", { name: "Redémarrer maintenant" })).toBeNull();
    expect(svc.identityCalls.map((c) => c.kind)).toEqual(["import"]);
  });
});
